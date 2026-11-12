//! Очередь воспроизведения: список, из которого запущен трек, и место в нём.
//!
//! Трек, запущенный из альбома, плейлиста или поиска, играет дальше по этому
//! списку, а не по «Мне нравится». Повтор и перемешивание работают по
//! очереди. Плееру очередь не нужна: он получает только звук трека.
//!
//! [`sound`] — единственное место, где трек для показа превращается в звук
//! для плеера. Здесь плагин FckCensor подменяет звук. Можно ли включить трек,
//! решает окно (`playable`): с заменой играет и недоступный трек.

use std::sync::Arc;

use crate::api::Track;
use crate::player::Sound;
use crate::plugins::fckcensor::{Censor, Decision};

/// Откуда список треков. По ключу окно находит список и понимает, что
/// строка в нём — это играющий трек.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListKey {
    Liked,
    /// «Мне нравится», отфильтрованный поиском по запросу.
    LikedFound(String),
    /// Страницы по номеру их записи в истории переходов.
    Album(u64),
    Playlist(u64),
    ArtistPopular(u64),
    ArtistTracks(u64),
    /// Треки поиска по номеру запроса.
    SearchTracks(u64),
    /// «Моя волна»: очередь растёт по мере догрузки.
    Wave,
    /// Очередь, отвязанная от списка: «Мне нравится» изменился (лайк или
    /// снятие), а трек продолжает играть по прежнему списку.
    Detached,
}

/// Простой генератор xorshift: для перемешивания его хватает.
pub struct Rng(u64);

impl Rng {
    /// Затравка — время запуска, не ноль.
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        Self(u64::from(nanos) | 1 << 63)
    }

    #[cfg(test)]
    fn seeded(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Число от 0 до `bound` включительно.
    fn up_to(&mut self, bound: usize) -> usize {
        let range = u64::try_from(bound).unwrap_or(u64::MAX).saturating_add(1);
        usize::try_from(self.next() % range).unwrap_or(0)
    }
}

/// Очередь: список, номер играющего трека и порядок перемешивания.
pub struct Queue {
    pub key: ListKey,
    pub tracks: Arc<Vec<Track>>,
    pub current: usize,
    /// Порядок треков при перемешивании (номера доступных треков).
    order: Vec<usize>,
}

impl Queue {
    pub fn new(key: ListKey, tracks: Arc<Vec<Track>>, current: usize) -> Self {
        Self {
            key,
            tracks,
            current,
            order: Vec::new(),
        }
    }

    pub fn track(&self) -> Option<&Track> {
        self.tracks.get(self.current)
    }

    pub fn get(&self, index: usize) -> Option<&Track> {
        self.tracks.get(index)
    }

    fn available(&self, index: usize, playable: &dyn Fn(&Track) -> bool) -> bool {
        self.tracks.get(index).is_some_and(playable)
    }

    /// Первый доступный трек: по списку или по новому порядку перемешивания.
    pub fn first(
        &mut self,
        shuffle: bool,
        rng: &mut Rng,
        playable: &dyn Fn(&Track) -> bool,
    ) -> Option<usize> {
        if shuffle {
            self.shuffle(None, rng, playable);
            return self.order.first().copied();
        }
        (0..self.tracks.len()).find(|index| self.available(*index, playable))
    }

    /// Новый порядок перемешивания доступных треков, `first` — в начало.
    pub fn shuffle(
        &mut self,
        first: Option<usize>,
        rng: &mut Rng,
        playable: &dyn Fn(&Track) -> bool,
    ) {
        let mut order: Vec<usize> = (0..self.tracks.len())
            .filter(|index| self.available(*index, playable))
            .collect();
        // Тасование Фишера — Йетса.
        for i in (1..order.len()).rev() {
            let j = rng.up_to(i);
            order.swap(i, j);
        }
        if let Some(first) = first
            && let Some(at) = order.iter().position(|index| *index == first)
        {
            order.swap(0, at);
        }
        self.order = order;
    }

    /// Список вырос (догрузилась страница): новые доступные треки встают в
    /// конец порядка перемешивания, вперемешку.
    pub fn extend(
        &mut self,
        tracks: Arc<Vec<Track>>,
        rng: &mut Rng,
        playable: &dyn Fn(&Track) -> bool,
    ) {
        let old = self.tracks.len();
        self.tracks = tracks;
        if self.order.is_empty() {
            return;
        }
        let mut fresh: Vec<usize> = (old..self.tracks.len())
            .filter(|index| self.available(*index, playable))
            .collect();
        for i in (1..fresh.len()).rev() {
            let j = rng.up_to(i);
            fresh.swap(i, j);
        }
        self.order.extend(fresh);
    }

    /// Какие треки пойдут после текущего (до `limit`): по порядку
    /// перемешивания, если он уже есть, иначе по списку. Только доступные, без
    /// повтора по кругу. Очередь не меняется.
    pub fn upcoming(
        &self,
        shuffle: bool,
        playable: &dyn Fn(&Track) -> bool,
        limit: usize,
    ) -> Vec<usize> {
        if shuffle && let Some(at) = self.order.iter().position(|index| *index == self.current) {
            return self
                .order
                .iter()
                .skip(at + 1)
                .take(limit)
                .copied()
                .collect();
        }
        (self.current.saturating_add(1)..self.tracks.len())
            .filter(|index| self.available(*index, playable))
            .take(limit)
            .collect()
    }

    /// Какие треки шли перед текущим (до `limit`), ближайший первым: по порядку
    /// перемешивания, если он есть, иначе по списку. Только доступные.
    pub fn before(
        &self,
        shuffle: bool,
        playable: &dyn Fn(&Track) -> bool,
        limit: usize,
    ) -> Vec<usize> {
        if shuffle && let Some(at) = self.order.iter().position(|index| *index == self.current) {
            return self
                .order
                .iter()
                .take(at)
                .rev()
                .take(limit)
                .copied()
                .collect();
        }
        (0..self.current.min(self.tracks.len()))
            .rev()
            .filter(|index| self.available(*index, playable))
            .take(limit)
            .collect()
    }

    /// Соседний доступный трек: по порядку перемешивания или по списку. С
    /// повтором списка (`wrap`) — по кругу.
    pub fn neighbour(
        &mut self,
        from: usize,
        forward: bool,
        shuffle: bool,
        wrap: bool,
        rng: &mut Rng,
        playable: &dyn Fn(&Track) -> bool,
    ) -> Option<usize> {
        if shuffle {
            if !self.order.contains(&from) {
                self.shuffle(Some(from), rng, playable);
            }
            let order = &self.order;
            let at = order.iter().position(|index| *index == from)?;
            let count = order.len();
            let next = match (forward, at) {
                (true, at) if at + 1 < count => at + 1,
                (true, _) if wrap => 0,
                (false, 0) if wrap => count.checked_sub(1)?,
                (false, at) if at > 0 => at - 1,
                _ => return None,
            };
            return order.get(next).copied();
        }
        let count = self.tracks.len();
        let mut index = from;
        for _ in 0..count {
            index = match (forward, index) {
                (true, i) if i + 1 < count => i + 1,
                (true, _) if wrap => 0,
                (false, 0) if wrap => count.saturating_sub(1),
                (false, i) if i > 0 => i - 1,
                _ => return None,
            };
            if self.available(index, playable) {
                return Some(index);
            }
        }
        None
    }
}

/// Какой звук играть вместо показанного трека. `censor` — FckCensor, если
/// он включён; `explicit_search` — искать ли версию без цензуры.
pub fn sound(track: &Track, censor: Option<&Censor>, explicit_search: bool) -> Decision {
    match censor {
        Some(censor) => censor.sound(track, explicit_search),
        None if track.available => Decision::Play(Sound::Track(track.id.clone())),
        None => Decision::Silent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(track: &Track) -> bool {
        track.available
    }

    fn tracks(available: &[bool]) -> Arc<Vec<Track>> {
        Arc::new(
            available
                .iter()
                .enumerate()
                .map(|(index, available)| Track {
                    id: index.to_string(),
                    title: None,
                    version: None,
                    artists: Vec::new(),
                    album_id: None,
                    album_title: None,
                    duration: None,
                    available: *available,
                    cover_uri: None,
                    warning: crate::api::Warning::None,
                    loudness: None,
                    lyrics: None,
                })
                .collect(),
        )
    }

    #[test]
    fn neighbours_skip_unavailable_and_wrap() {
        let mut rng = Rng::seeded(7);
        let mut queue = Queue::new(ListKey::Album(1), tracks(&[true, false, true, true]), 0);
        assert_eq!(
            queue.neighbour(0, true, false, false, &mut rng, &ok),
            Some(2)
        );
        assert_eq!(queue.neighbour(3, true, false, false, &mut rng, &ok), None);
        assert_eq!(
            queue.neighbour(3, true, false, true, &mut rng, &ok),
            Some(0)
        );
        assert_eq!(
            queue.neighbour(0, false, false, true, &mut rng, &ok),
            Some(3)
        );
        assert_eq!(
            queue.neighbour(2, false, false, false, &mut rng, &ok),
            Some(0)
        );
        assert_eq!(queue.first(false, &mut rng, &ok), Some(0));
    }

    #[test]
    fn upcoming_follows_list_and_skips_unavailable() {
        let queue = Queue::new(
            ListKey::Album(1),
            tracks(&[true, true, false, true, true]),
            1,
        );
        assert_eq!(queue.upcoming(false, &ok, 10), vec![3, 4]);
        assert_eq!(queue.upcoming(false, &ok, 1), vec![3]);
        let last = Queue::new(ListKey::Album(1), tracks(&[true, true]), 1);
        assert!(last.upcoming(false, &ok, 10).is_empty());
        let empty = Queue::new(ListKey::Liked, tracks(&[]), 0);
        assert!(empty.upcoming(false, &ok, 10).is_empty());
    }

    #[test]
    fn before_lists_played_tracks_closest_first() {
        let queue = Queue::new(
            ListKey::Album(1),
            tracks(&[true, false, true, true, true]),
            3,
        );
        assert_eq!(queue.before(false, &ok, 10), vec![2, 0]);
        assert_eq!(queue.before(false, &ok, 1), vec![2]);
        let first = Queue::new(ListKey::Album(1), tracks(&[true, true]), 0);
        assert!(first.before(false, &ok, 10).is_empty());
        // Перемешивание: то, что стояло в порядке до текущего.
        let mut rng = Rng::seeded(5);
        let mut shuffled = Queue::new(ListKey::Liked, tracks(&[true; 6]), 2);
        shuffled.shuffle(Some(2), &mut rng, &ok);
        shuffled.current = *shuffled.order.get(3).unwrap();
        let expected: Vec<usize> = shuffled.order.iter().take(3).rev().copied().collect();
        assert_eq!(shuffled.before(true, &ok, 10), expected);
    }

    #[test]
    fn upcoming_follows_shuffle_order() {
        let mut rng = Rng::seeded(11);
        let mut queue = Queue::new(ListKey::Liked, tracks(&[true; 8]), 3);
        queue.shuffle(Some(3), &mut rng, &ok);
        let order = queue.order.clone();
        // Текущий первый в порядке: дальше весь остальной порядок.
        assert_eq!(queue.upcoming(true, &ok, 100), order.get(1..).unwrap());
        assert_eq!(queue.upcoming(true, &ok, 3).len(), 3);
        // Перемешивание не начато (порядка нет): показываем по списку.
        let plain = Queue::new(ListKey::Liked, tracks(&[true; 4]), 0);
        assert_eq!(plain.upcoming(true, &ok, 10), vec![1, 2, 3]);
        // Середина порядка: только то, что после текущего.
        queue.current = *order.get(4).unwrap();
        assert_eq!(queue.upcoming(true, &ok, 100), order.get(5..).unwrap());
    }

    #[test]
    fn nothing_available_means_no_neighbour() {
        let mut rng = Rng::seeded(1);
        let mut queue = Queue::new(ListKey::Liked, tracks(&[false, false]), 0);
        assert_eq!(queue.neighbour(0, true, false, true, &mut rng, &ok), None);
        assert_eq!(queue.first(false, &mut rng, &ok), None);
        assert_eq!(queue.first(true, &mut rng, &ok), None);
        let mut empty = Queue::new(ListKey::Liked, tracks(&[]), 0);
        assert_eq!(empty.neighbour(0, true, true, true, &mut rng, &ok), None);
    }

    #[test]
    fn shuffle_visits_every_available_track_once() {
        let mut rng = Rng::seeded(42);
        let mut queue = Queue::new(ListKey::Liked, tracks(&[true; 50]), 0);
        queue.shuffle(Some(10), &mut rng, &ok);
        let mut seen = vec![10];
        let mut at = 10;
        while let Some(next) = queue.neighbour(at, true, true, false, &mut rng, &ok) {
            seen.push(next);
            at = next;
        }
        seen.sort_unstable();
        assert_eq!(seen, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn extended_queue_keeps_order_and_adds_new_tracks() {
        let mut rng = Rng::seeded(3);
        let mut queue = Queue::new(ListKey::SearchTracks(1), tracks(&[true; 4]), 0);
        queue.shuffle(Some(0), &mut rng, &ok);
        let before = queue.order.clone();
        queue.extend(
            tracks(&[true, true, true, true, true, false, true]),
            &mut rng,
            &ok,
        );
        assert_eq!(queue.order.get(..4), Some(before.as_slice()));
        let mut added: Vec<usize> = queue.order.get(4..).unwrap().to_vec();
        added.sort_unstable();
        assert_eq!(added, [4, 6]);
    }

    #[test]
    fn unavailable_track_has_no_sound() {
        let list = tracks(&[true, false]);
        assert_eq!(
            sound(&list[0], None, true),
            Decision::Play(Sound::Track("0".to_owned()))
        );
        assert_eq!(sound(&list[1], None, true), Decision::Silent);
    }
    fn censor_with_list() -> Censor {
        use crate::plugins::fckcensor::{Loaded, SAMPLE_LIST, parse_list};
        let mut censor = Censor::default();
        censor.loaded(
            Loaded {
                list: Arc::new(parse_list(SAMPLE_LIST.as_bytes()).unwrap()),
                fetched: 0,
                stale: None,
            },
            None,
        );
        censor
    }

    #[test]
    fn censor_replaces_sound_and_opens_unavailable_tracks() {
        use crate::plugins::fckcensor::Replacement;
        let mut censor = censor_with_list();
        let mut list: Vec<Track> = tracks(&[true, false, false, true])
            .iter()
            .cloned()
            .collect();
        // Номера из примера списка: «2» — снят, но у него есть замена в списке.
        if let Some(track) = list.get_mut(1) {
            track.id = "2".to_owned();
        }
        if let Some(track) = list.get_mut(2) {
            track.id = "99".to_owned();
        }
        let list = Arc::new(list);

        let first = list.first().unwrap();
        assert_eq!(
            sound(first, Some(&censor), false),
            Decision::Play(Sound::Track("0".to_owned()))
        );
        let listed = list.get(1).unwrap();
        assert_eq!(
            sound(listed, Some(&censor), false),
            Decision::Play(Sound::Track("29238706".to_owned()))
        );
        let gone = list.get(2).unwrap();
        assert_eq!(sound(gone, Some(&censor), false), Decision::Silent);
        // Выключенный плагин (censor = None) замен не знает.
        assert_eq!(sound(listed, None, false), Decision::Silent);

        // Очередь с FckCensor доходит до трека со звуком-заменой.
        let mut rng = Rng::seeded(5);
        let mut queue = Queue::new(ListKey::Liked, list.clone(), 0);
        let plain = |track: &Track| censor_playable(&censor_with_list(), track);
        assert_eq!(
            queue.neighbour(0, true, false, false, &mut rng, &plain),
            Some(1)
        );
        assert_eq!(
            queue.neighbour(1, true, false, false, &mut rng, &plain),
            Some(3)
        );
        // Без замены трека «2» очередь его перескакивает.
        assert_eq!(
            queue.neighbour(0, true, false, false, &mut rng, &ok),
            Some(3)
        );

        // Ручная замена файлом тоже открывает трек.
        censor
            .replacements
            .tracks
            .insert("1".to_owned(), Replacement::file("мой-файл.mp3".into()));
        let mut hidden = first.clone();
        hidden.id = "1".to_owned();
        hidden.available = false;
        assert_eq!(
            sound(&hidden, Some(&censor), false),
            Decision::Play(Sound::File("мой-файл.mp3".into()))
        );
    }

    fn censor_playable(censor: &Censor, track: &Track) -> bool {
        censor.playable(track)
    }
}
