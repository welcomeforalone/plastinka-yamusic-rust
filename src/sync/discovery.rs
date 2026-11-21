//! Обнаружение устройств по mDNS / DNS-SD (§2 спеки): объявление службы
//! `_plastinka._tcp.local.` и поиск других устройств с тем же аккаунтом.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo, instance_name_unescaped};
use tokio::sync::mpsc::UnboundedSender;

use super::proto::is_device_id;
use super::{PROTOCOL_VERSION, SyncError};

/// Тип службы.
pub const SERVICE_TYPE: &str = "_plastinka._tcp.local.";
/// Предел имени экземпляра, байт UTF-8.
const MAX_INSTANCE_BYTES: usize = 63;
/// Сколько ждём остановку демона при выходе.
const SHUTDOWN_WAIT: Duration = Duration::from_millis(300);

/// Устройство, найденное в сети.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visible {
    pub id: String,
    pub name: String,
    pub os: String,
    /// IPv4 с портом, частные диапазоны первыми.
    pub addrs: Vec<SocketAddr>,
    /// Полное имя записи mDNS (для исчезновения).
    pub fullname: String,
}

/// Имя экземпляра не длиннее 63 байт UTF-8 (по границе символа).
pub fn instance_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if out.len() + ch.len_utf8() > MAX_INSTANCE_BYTES {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        "plastinka".to_owned()
    } else {
        out
    }
}

/// Частные адреса (10/8, 172.16/12, 192.168/16) первыми, остальные следом;
/// внутри групп по возрастанию (порядок стабилен).
pub fn sort_addrs(mut addrs: Vec<Ipv4Addr>) -> Vec<Ipv4Addr> {
    addrs.sort_by_key(|addr| (!addr.is_private(), *addr));
    addrs.dedup();
    addrs
}

/// Запись mDNS → устройство. Пропускаются молча: чужая версия, чужой
/// аккаунт, свой `id`, пустой или неверный TXT, нет IPv4.
pub fn visible_from(
    fullname: &str,
    port: u16,
    addrs: Vec<Ipv4Addr>,
    txt: &dyn Fn(&str) -> Option<String>,
    own_id: &str,
    own_acc: &str,
) -> Option<Visible> {
    if txt("v")?.trim() != PROTOCOL_VERSION.to_string() {
        return None;
    }
    let id = txt("id")?;
    let acc = txt("acc")?;
    let os = txt("os")?;
    if !is_device_id(&id) || id == own_id || acc != own_acc || os.is_empty() || port == 0 {
        return None;
    }
    let addrs: Vec<SocketAddr> = sort_addrs(addrs)
        .into_iter()
        .map(|ip| SocketAddr::from((ip, port)))
        .collect();
    if addrs.is_empty() {
        return None;
    }
    let name = crate::api::clean(&instance_name_unescaped(fullname))
        .map(|name| name.chars().take(64).collect())
        .unwrap_or_else(|| "Устройство".to_owned());
    Some(Visible {
        id,
        name,
        os: os.chars().take(16).collect(),
        addrs,
        fullname: fullname.to_owned(),
    })
}

/// Таблица видимых устройств.
#[derive(Debug, Default)]
pub struct DeviceTable {
    by_fullname: HashMap<String, Visible>,
}

impl DeviceTable {
    /// Обновляет запись. `true`, если таблица изменилась.
    pub fn upsert(&mut self, device: Visible) -> bool {
        let changed = self.by_fullname.get(&device.fullname) != Some(&device);
        self.by_fullname.insert(device.fullname.clone(), device);
        changed
    }

    /// Убирает запись по полному имени. `true`, если она была.
    pub fn remove(&mut self, fullname: &str) -> bool {
        self.by_fullname.remove(fullname).is_some()
    }

    /// Список по имени (затем по `id`), одно устройство — одна запись.
    pub fn snapshot(&self) -> Vec<Visible> {
        let mut by_id: HashMap<&str, &Visible> = HashMap::new();
        for device in self.by_fullname.values() {
            by_id.entry(device.id.as_str()).or_insert(device);
        }
        let mut list: Vec<Visible> = by_id.into_values().cloned().collect();
        list.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
        list
    }
}

/// Что объявляем о себе.
#[derive(Debug, Clone)]
pub struct Announce {
    pub id: String,
    pub name: String,
    pub os: String,
    pub acc: String,
    pub port: u16,
}

/// Объявление и поиск. Остановка — при удалении.
pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
}

fn failure(err: &mdns_sd::Error) -> SyncError {
    SyncError::Discovery(err.to_string())
}

impl Discovery {
    /// Объявляет это устройство и ищет другие. Снимки таблицы приходят в
    /// `updates` при каждом изменении.
    pub fn start(
        announce: &Announce,
        updates: UnboundedSender<Vec<Visible>>,
    ) -> Result<Self, SyncError> {
        let daemon = ServiceDaemon::new().map_err(|err| failure(&err))?;
        let props: HashMap<String, String> = HashMap::from([
            ("v".to_owned(), PROTOCOL_VERSION.to_string()),
            ("id".to_owned(), announce.id.clone()),
            ("acc".to_owned(), announce.acc.clone()),
            ("os".to_owned(), announce.os.clone()),
        ]);
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name(&announce.name),
            &format!("{}.local.", announce.id),
            (),
            announce.port,
            props,
        )
        .map_err(|err| failure(&err))?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_owned();
        daemon.register(info).map_err(|err| failure(&err))?;
        let receiver = daemon.browse(SERVICE_TYPE).map_err(|err| failure(&err))?;

        let (own_id, own_acc) = (announce.id.clone(), announce.acc.clone());
        std::thread::Builder::new()
            .name("plastinka-mdns".to_owned())
            .spawn(move || {
                let mut table = DeviceTable::default();
                // Цикл кончается, когда демон остановлен и канал закрыт.
                while let Ok(event) = receiver.recv() {
                    let changed = match event {
                        ServiceEvent::ServiceResolved(info) => {
                            let txt = |key: &str| info.get_property_val_str(key).map(str::to_owned);
                            visible_from(
                                info.get_fullname(),
                                info.get_port(),
                                info.get_addresses_v4().into_iter().collect(),
                                &txt,
                                &own_id,
                                &own_acc,
                            )
                            .is_some_and(|device| table.upsert(device))
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => table.remove(&fullname),
                        _ => false,
                    };
                    if changed && updates.send(table.snapshot()).is_err() {
                        break;
                    }
                }
            })
            .map_err(|err| SyncError::Discovery(err.to_string()))?;
        Ok(Self { daemon, fullname })
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        // Снимаем объявление и останавливаем демон, но недолго: выход не
        // должен зависать.
        if let Ok(done) = self.daemon.unregister(&self.fullname) {
            let _ = done.recv_timeout(SHUTDOWN_WAIT);
        }
        if let Ok(done) = self.daemon.shutdown() {
            let _ = done.recv_timeout(SHUTDOWN_WAIT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
    const ME: &str = "efefefefefefefefefefefefefefefef";

    fn txt<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn ip(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr::new(a, b, c, d)
    }

    #[test]
    fn instance_name_is_cut_on_char_boundary() {
        assert_eq!(instance_name("Телефон"), "Телефон");
        let long = "ы".repeat(60);
        let cut = instance_name(&long);
        assert!(cut.len() <= 63 && cut.chars().all(|c| c == 'ы'));
        assert_eq!(cut.len(), 62);
        assert_eq!(instance_name(""), "plastinka");
    }

    #[test]
    fn private_addresses_come_first() {
        let sorted = sort_addrs(vec![
            ip(8, 8, 8, 8),
            ip(192, 168, 1, 5),
            ip(10, 0, 0, 2),
            ip(172, 20, 0, 1),
            ip(192, 168, 1, 5),
            ip(172, 32, 0, 1),
        ]);
        assert_eq!(
            sorted,
            [
                ip(10, 0, 0, 2),
                ip(172, 20, 0, 1),
                ip(192, 168, 1, 5),
                ip(8, 8, 8, 8),
                ip(172, 32, 0, 1)
            ]
        );
    }

    #[test]
    fn records_are_filtered() {
        let good = [
            ("v", "1"),
            ("id", ID),
            ("acc", "0123456789ab"),
            ("os", "android"),
        ];
        let make = |pairs: &[(&str, &str)], id: &str, acc: &str, port: u16| {
            visible_from(
                "Телефон._plastinka._tcp.local.",
                port,
                vec![ip(192, 168, 0, 7)],
                &txt(pairs),
                id,
                acc,
            )
        };
        let device = make(&good, ME, "0123456789ab", 47800).unwrap();
        assert_eq!(device.id, ID);
        assert_eq!(device.name, "Телефон");
        assert_eq!(
            device.addrs,
            [SocketAddr::from((ip(192, 168, 0, 7), 47800))]
        );
        // Чужой аккаунт, свой id, чужая версия, пустой и битый TXT, нет порта.
        assert!(make(&good, ME, "ffffffffffff", 47800).is_none());
        assert!(make(&good, ID, "0123456789ab", 47800).is_none());
        assert!(make(&good, ME, "0123456789ab", 0).is_none());
        assert!(
            make(
                &[("v", "2"), ("id", ID), ("acc", "0123456789ab"), ("os", "x")],
                ME,
                "0123456789ab",
                1
            )
            .is_none()
        );
        assert!(
            make(
                &[
                    ("v", "1"),
                    ("id", "short"),
                    ("acc", "0123456789ab"),
                    ("os", "x")
                ],
                ME,
                "0123456789ab",
                1
            )
            .is_none()
        );
        assert!(make(&[], ME, "0123456789ab", 1).is_none());
    }

    #[test]
    fn table_tracks_appearance_and_removal() {
        let device = |fullname: &str, name: &str| Visible {
            id: ID.into(),
            name: name.into(),
            os: "linux".into(),
            addrs: vec![SocketAddr::from((ip(10, 0, 0, 1), 1))],
            fullname: fullname.into(),
        };
        let mut table = DeviceTable::default();
        assert!(table.upsert(device("a", "A")));
        assert!(!table.upsert(device("a", "A")));
        // Одно устройство под двумя именами записей — одна строка.
        assert!(table.upsert(device("a (2)", "A")));
        assert_eq!(table.snapshot().len(), 1);
        assert!(table.remove("a"));
        assert!(!table.remove("a"));
        assert_eq!(table.snapshot().len(), 1);
        assert!(table.remove("a (2)"));
        assert!(table.snapshot().is_empty());
    }
}
