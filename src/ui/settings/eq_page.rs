//! Страница «Эквалайзер»: переключатель, пресеты, полосы, предусилитель.
//! Логика «сообщение → новое состояние» — чистая функция [`update`]: окно
//! только вызывает её и по результату отправляет параметры плееру и пишет
//! `config.toml`.

use std::borrow::Cow;

use iced::widget::{Space, button, column, container, row};
use iced::{Alignment, Border, Element, Length, Padding};

use super::controls::{self, Field, switch};
use super::eq_bands::{self, format_db};
use super::pages::item;
use super::{Ctx, Entry, Msg as SettingsMsg};
use crate::player::eq::{
    BANDS, EqState, GAIN_LIMIT, PRESETS, PresetError, PresetId, preset_by_key,
};
use crate::ui::Message as App;
use crate::ui::icons::{Icon, icon};
use crate::ui::theme::{self, Style};
use crate::ui::widgets::{self, Kind};

/// Поле имени своего пресета (для фокуса при открытии).
pub const NAME_ID: &str = "settings-eq-name";
/// Шаг предусилителя: 0,5 дБ в десятых долях.
const PREAMP_STEP: i16 = 5;
/// Ширина строки с ползунком: значение, промежуток, дорожка.
const SLIDER_ROW: f32 = 384.0;
/// Окно уже этого — ползунок предусилителя уходит под заголовок строки.
const STACK_BELOW: f32 = 1240.0;

/// Сообщения страницы.
#[derive(Debug, Clone)]
pub enum Msg {
    Toggle(bool),
    /// Полоса (номер, десятые доли дБ): перетаскивание или щелчок.
    Band(usize, i16),
    Preamp(i16),
    AutoPreamp(bool),
    Preset(PresetId),
    ResetBands,
    /// Закончили менять полосу или предусилитель: записать в `config.toml`.
    Commit,
    /// Раскрыть поле имени своего пресета.
    NameOpen,
    NameCancel,
    NameInput(String),
    SavePreset,
    RemovePreset(String),
}

/// Поле имени для «Сохранить как…». В `config.toml` не пишется.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameField {
    pub open: bool,
    pub text: String,
    pub error: Option<PresetError>,
}

impl NameField {
    fn close(&mut self) {
        *self = Self::default();
    }
}

/// Что окну сделать после сообщения.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effect {
    /// Параметры изменились: отправить плееру.
    pub apply: bool,
    /// Записать в `config.toml`.
    pub save: bool,
}

const APPLY_AND_SAVE: Effect = Effect {
    apply: true,
    save: true,
};
const APPLY_ONLY: Effect = Effect {
    apply: true,
    save: false,
};
const SAVE_ONLY: Effect = Effect {
    apply: false,
    save: true,
};

/// После ручной правки выбранный пресет остаётся, пока значения с ним
/// совпадают; иначе берём тот, с которым совпали (или «Свои значения»).
fn refresh_preset(eq: &mut EqState) {
    let keep = eq.preset.as_ref().is_some_and(|id| eq.preset_matches(id));
    if !keep {
        eq.preset = eq.matching_preset();
    }
}

/// Применяет сообщение к состоянию.
pub fn update(eq: &mut EqState, name: &mut NameField, msg: Msg) -> Effect {
    match msg {
        Msg::Toggle(on) => {
            eq.settings.enabled = on;
            APPLY_AND_SAVE
        }
        Msg::Band(index, value) => {
            let Some(slot) = eq.settings.gains.get_mut(index) else {
                return Effect::default();
            };
            *slot = value.clamp(-GAIN_LIMIT, GAIN_LIMIT);
            refresh_preset(eq);
            APPLY_ONLY
        }
        Msg::Preamp(value) => {
            eq.settings.preamp = value.clamp(-GAIN_LIMIT, GAIN_LIMIT);
            refresh_preset(eq);
            APPLY_ONLY
        }
        Msg::AutoPreamp(on) => {
            eq.settings.auto_preamp = on;
            APPLY_AND_SAVE
        }
        Msg::Preset(id) => {
            if !eq.apply_preset(&id) {
                return Effect::default();
            }
            // Выбрать пресет — значит хотеть его услышать.
            eq.settings.enabled = true;
            APPLY_AND_SAVE
        }
        Msg::ResetBands => {
            eq.settings.gains = [0; BANDS];
            eq.settings.preamp = 0;
            eq.preset = preset_by_key("flat").map(|preset| PresetId::Builtin(preset.key));
            APPLY_AND_SAVE
        }
        Msg::Commit => SAVE_ONLY,
        Msg::NameOpen => {
            name.close();
            name.open = true;
            Effect::default()
        }
        Msg::NameCancel => {
            name.close();
            Effect::default()
        }
        Msg::NameInput(text) => {
            name.text = text;
            name.error = None;
            Effect::default()
        }
        Msg::SavePreset => match eq.save_current_as(&name.text) {
            Ok(()) => {
                name.close();
                SAVE_ONLY
            }
            Err(error) => {
                name.error = Some(error);
                Effect::default()
            }
        },
        Msg::RemovePreset(preset) => {
            eq.remove_custom(&preset);
            SAVE_ONLY
        }
    }
}

/// Предусилитель из ползунка (дБ) в десятые доли с шагом 0,5.
fn preamp_from_slider(value: f32) -> i16 {
    if !value.is_finite() {
        return 0;
    }
    let limit = f32::from(GAIN_LIMIT);
    let tenths = (value * 10.0).clamp(-limit, limit);
    let step = f32::from(PREAMP_STEP);
    ((tenths / step).round() * step) as i16
}

fn msg(message: Msg) -> App {
    App::Settings(SettingsMsg::Eq(message))
}

// ------------------------------------------------------------------- вид

/// Записи страницы по порядку.
pub fn entries<'a>(ctx: &Ctx<'a>) -> Vec<Entry<'a>> {
    let eq = ctx.eq;
    let settings = eq.settings;
    let side_by_side = ctx.width >= STACK_BELOW;

    let toggle = item(
        "Эквалайзер",
        "Меняет звук на лету, на качество потока не влияет".to_owned(),
        "звук бас полосы пресет включить",
        switch(settings.enabled, |on| msg(Msg::Toggle(on))),
    );

    let presets = Entry::new(
        "Пресет",
        selected_title(eq),
        "бас пресет полосы вокал рок поп джаз сохранить",
        container(presets_view(eq, ctx.eq_name))
            .padding(Padding::from([16, 0]))
            .into(),
    );

    let bands = Entry::new(
        "Полосы",
        "От 32 Гц до 16 кГц, шаг 0,5 дБ",
        "бас частоты полосы усиление",
        container(eq_bands::view(
            settings.gains,
            settings.enabled,
            |index, value| msg(Msg::Band(index, value)),
            msg(Msg::Commit),
        ))
        .padding(Padding::from([16, 0]))
        .into(),
    );

    let preamp_hint = if settings.auto_preamp {
        format!(
            "Общий уровень до полос. С учётом автоматического: {}",
            format_db(settings.effective_preamp())
        )
    } else {
        "Общий уровень до полос".to_owned()
    };
    let slider = controls::value_slider(
        format_db(settings.preamp),
        -f32::from(GAIN_LIMIT) / 10.0..=f32::from(GAIN_LIMIT) / 10.0,
        f32::from(settings.preamp) / 10.0,
        0.5,
        |value| msg(Msg::Preamp(preamp_from_slider(value))),
        Some(msg(Msg::Commit)),
    );
    let preamp = if side_by_side {
        item(
            "Предусилитель",
            preamp_hint.clone(),
            "звук громкость уровень предусилитель",
            container(slider).width(SLIDER_ROW).into(),
        )
    } else {
        // В узком окне ползунок под заголовком на всю ширину.
        Entry::new(
            "Предусилитель",
            preamp_hint.clone(),
            "звук громкость уровень предусилитель",
            column![
                theme::line("Предусилитель", Style::BodyStrong, theme::text_primary()),
                theme::label(preamp_hint, Style::Secondary, theme::text_tertiary()),
                Space::new().height(6),
                container(slider).width(Length::Fill).max_width(SLIDER_ROW),
            ]
            .spacing(2)
            .padding(Padding::from([16, 0]))
            .into(),
        )
    };

    let auto = item(
        "Автоматический предусилитель",
        "Снижает общий уровень на величину наибольшего усиления, чтобы звук не искажался"
            .to_owned(),
        "звук громкость уровень предусилитель искажения",
        switch(settings.auto_preamp, |on| msg(Msg::AutoPreamp(on))),
    );

    let flat = is_flat(eq);
    let reset = Entry::new(
        "Сбросить полосы",
        "Все полосы и предусилитель в ноль, пресет «Плоский»",
        "сброс ноль плоский полосы",
        container(widgets::action(
            Kind::Secondary,
            None,
            "Сбросить полосы",
            (!flat).then(|| msg(Msg::ResetBands)),
        ))
        .width(Length::Fill)
        .align_x(Alignment::End)
        .padding(Padding::from([16, 0]))
        .into(),
    );

    vec![toggle, presets, bands, preamp, auto, reset]
}

/// Полосы и предусилитель уже в нуле, выбран «Плоский».
fn is_flat(eq: &EqState) -> bool {
    eq.settings.gains.iter().all(|gain| *gain == 0)
        && eq.settings.preamp == 0
        && eq.preset == Some(PresetId::Builtin("flat"))
}

/// Название выбранного пресета для пояснения строки.
fn selected_title(eq: &EqState) -> Cow<'static, str> {
    match &eq.preset {
        Some(PresetId::Builtin(key)) => preset_by_key(key)
            .map_or(Cow::Borrowed("Свои значения"), |preset| {
                Cow::Borrowed(preset.title)
            }),
        Some(PresetId::Custom(name)) => Cow::Owned(name.clone()),
        None => Cow::Borrowed("Свои значения"),
    }
}

/// «Таблетки» пресетов с переносом строк, под ними поле имени.
fn presets_view<'a>(eq: &'a EqState, name: &'a NameField) -> Element<'a, App> {
    let mut pills: Vec<Element<'a, App>> = Vec::new();
    for preset in PRESETS {
        let id = PresetId::Builtin(preset.key);
        let selected = eq.preset.as_ref() == Some(&id);
        pills.push(pill(preset.title, selected, msg(Msg::Preset(id))));
    }
    for custom in &eq.custom {
        let id = PresetId::Custom(custom.name.clone());
        let selected = eq.preset.as_ref() == Some(&id);
        pills.push(custom_pill(&custom.name, selected));
    }
    pills.push(add_pill(name.open));
    let wrapped = row(pills).spacing(8).wrap().vertical_spacing(8);

    let mut content = column![
        theme::line("Пресет", Style::BodyStrong, theme::text_primary()),
        wrapped,
    ]
    .spacing(12);
    if name.open {
        content = content.push(name_form(name));
    }
    content.into()
}

/// Цвета «таблетки»: фон и подпись.
fn pill_colors(selected: bool, status: button::Status) -> (iced::Color, iced::Color) {
    match (selected, status) {
        (true, _) => (theme::accent(), theme::on_accent()),
        (false, button::Status::Hovered | button::Status::Pressed) => {
            (theme::surface_4(), theme::text_primary())
        }
        (false, _) => (theme::surface_3(), theme::text_secondary()),
    }
}

fn pill<'a>(title: &'a str, selected: bool, on_press: App) -> Element<'a, App> {
    button(theme::line(
        title,
        Style::CaptionStrong,
        pill_colors(selected, button::Status::Active).1,
    ))
    .padding([6, 14])
    .style(move |_, status| {
        let (background, text_color) = pill_colors(selected, status);
        button::Style {
            background: Some(background.into()),
            text_color,
            border: Border::default().rounded(widgets::ROUND),
            ..button::Style::default()
        }
    })
    .on_press(on_press)
    .into()
}

/// Свой пресет: название и крестик «удалить» в одной «таблетке».
fn custom_pill<'a>(name: &'a str, selected: bool) -> Element<'a, App> {
    let (background, color) = pill_colors(selected, button::Status::Active);
    let choose = button(theme::line(name, Style::CaptionStrong, color))
        .padding(Padding {
            top: 6.0,
            right: 4.0,
            bottom: 6.0,
            left: 14.0,
        })
        .style(move |_, _| button::Style {
            text_color: color,
            ..button::Style::default()
        })
        .on_press(msg(Msg::Preset(PresetId::Custom(name.to_owned()))));
    let remove = button(icon(Icon::Close, 14.0, color))
        .padding(Padding {
            top: 6.0,
            right: 10.0,
            bottom: 6.0,
            left: 4.0,
        })
        .style(move |_, _| button::Style {
            text_color: color,
            ..button::Style::default()
        })
        .on_press(msg(Msg::RemovePreset(name.to_owned())));
    container(row![choose, remove].align_y(Alignment::Center))
        .style(move |_| container::Style {
            background: Some(background.into()),
            border: Border::default().rounded(widgets::ROUND),
            ..container::Style::default()
        })
        .into()
}

/// «Сохранить как…»: раскрывает поле имени.
fn add_pill<'a>(open: bool) -> Element<'a, App> {
    button(
        row![
            icon(Icon::Plus, 14.0, theme::text_secondary()),
            theme::line(
                "Сохранить как…",
                Style::CaptionStrong,
                theme::text_secondary()
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([6, 14])
    .style(move |_, status| button::Style {
        background: None,
        text_color: theme::text_secondary(),
        border: Border {
            color: match (open, status) {
                (true, _) => theme::accent(),
                (false, button::Status::Hovered | button::Status::Pressed) => {
                    theme::text_secondary()
                }
                _ => theme::outline(),
            },
            width: 1.0,
            radius: widgets::ROUND.into(),
        },
        ..button::Style::default()
    })
    .on_press(msg(if open { Msg::NameCancel } else { Msg::NameOpen }))
    .into()
}

/// Поле имени, «Сохранить» и подсказка об ошибке под ним.
fn name_form<'a>(name: &'a NameField) -> Element<'a, App> {
    let field = controls::text_field(
        Field {
            id: Some(NAME_ID),
            placeholder: "Название пресета",
            value: &name.text,
            icon: None,
            width: Length::Fill,
        },
        |text| msg(Msg::NameInput(text)),
        None,
    );
    let mut form = column![
        row![
            container(field).max_width(360),
            widgets::action(Kind::Primary, None, "Сохранить", Some(msg(Msg::SavePreset))),
            widgets::action(Kind::Secondary, None, "Отмена", Some(msg(Msg::NameCancel))),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(6);
    if let Some(error) = name.error {
        form = form.push(theme::label(
            error.to_string(),
            Style::Caption,
            theme::error(),
        ));
    }
    form.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(eq: &mut EqState, msg: Msg) -> Effect {
        update(eq, &mut NameField::default(), msg)
    }

    fn rock() -> [i16; BANDS] {
        [40, 30, 20, 0, -10, -10, 10, 30, 40, 40]
    }

    #[test]
    fn band_drag_applies_without_saving() {
        let mut eq = EqState::default();
        let effect = run(&mut eq, Msg::Band(3, 45));
        assert_eq!(
            effect,
            Effect {
                apply: true,
                save: false
            }
        );
        assert_eq!(eq.settings.gains[3], 45);
        // Commit записывает, но плееру ничего нового не шлёт.
        assert_eq!(
            run(&mut eq, Msg::Commit),
            Effect {
                apply: false,
                save: true
            }
        );
    }

    #[test]
    fn band_out_of_range_is_ignored_and_clamped() {
        let mut eq = EqState::default();
        assert_eq!(run(&mut eq, Msg::Band(BANDS, 10)), Effect::default());
        assert_eq!(eq, EqState::default());
        run(&mut eq, Msg::Band(0, 500));
        assert_eq!(eq.settings.gains[0], GAIN_LIMIT);
        run(&mut eq, Msg::Preamp(-500));
        assert_eq!(eq.settings.preamp, -GAIN_LIMIT);
    }

    #[test]
    fn manual_change_drops_preset_and_restores_when_equal() {
        let mut eq = EqState::default();
        run(&mut eq, Msg::Preset(PresetId::Builtin("rock")));
        assert_eq!(eq.preset, Some(PresetId::Builtin("rock")));
        assert_eq!(eq.settings.gains, rock());
        // Сдвинули полосу — «Свои значения».
        run(&mut eq, Msg::Band(0, 45));
        assert_eq!(eq.preset, None);
        // Вернули — пресет снова выбран.
        run(&mut eq, Msg::Band(0, 40));
        assert_eq!(eq.preset, Some(PresetId::Builtin("rock")));
    }

    #[test]
    fn preamp_change_keeps_builtin_but_drops_custom() {
        let mut eq = EqState::default();
        run(&mut eq, Msg::Preset(PresetId::Builtin("rock")));
        run(&mut eq, Msg::Preamp(-30));
        assert_eq!(eq.preset, Some(PresetId::Builtin("rock")));
        run(&mut eq, Msg::Preamp(0));
        let mut name = NameField {
            text: "Мой".to_owned(),
            ..NameField::default()
        };
        update(&mut eq, &mut name, Msg::SavePreset);
        assert_eq!(eq.preset, Some(PresetId::Custom("Мой".to_owned())));
        // Предусилитель у своего пресета часть пресета.
        run(&mut eq, Msg::Preamp(-30));
        assert_eq!(eq.preset, Some(PresetId::Builtin("rock")));
        run(&mut eq, Msg::Preamp(0));
        assert_eq!(eq.preset, Some(PresetId::Builtin("rock")));
    }

    #[test]
    fn choosing_preset_enables_equalizer_and_saves() {
        let mut eq = EqState::default();
        assert!(!eq.settings.enabled);
        let effect = run(&mut eq, Msg::Preset(PresetId::Builtin("bass")));
        assert_eq!(effect, APPLY_AND_SAVE);
        assert!(eq.settings.enabled);
        assert_eq!(eq.settings.gains, [60, 50, 40, 20, 0, 0, 0, 0, 0, 0]);
        // Неизвестный пресет ничего не меняет.
        let before = eq.clone();
        assert_eq!(
            run(&mut eq, Msg::Preset(PresetId::Custom("нет".to_owned()))),
            Effect::default()
        );
        assert_eq!(eq, before);
    }

    #[test]
    fn reset_bands_zeroes_and_selects_flat() {
        let mut eq = EqState::default();
        eq.settings.enabled = true;
        run(&mut eq, Msg::Preset(PresetId::Builtin("rock")));
        run(&mut eq, Msg::Preamp(-40));
        let effect = run(&mut eq, Msg::ResetBands);
        assert_eq!(effect, APPLY_AND_SAVE);
        assert_eq!(eq.settings.gains, [0; BANDS]);
        assert_eq!(eq.settings.preamp, 0);
        assert_eq!(eq.preset, Some(PresetId::Builtin("flat")));
        // Включённость не трогаем.
        assert!(eq.settings.enabled);
        assert!(is_flat(&eq));
    }

    #[test]
    fn toggles_save_and_apply() {
        let mut eq = EqState::default();
        assert_eq!(run(&mut eq, Msg::Toggle(true)), APPLY_AND_SAVE);
        assert!(eq.settings.enabled);
        assert_eq!(run(&mut eq, Msg::AutoPreamp(false)), APPLY_AND_SAVE);
        assert!(!eq.settings.auto_preamp);
    }

    #[test]
    fn name_errors_are_reported_and_keep_field_open() {
        let mut eq = EqState::default();
        let mut name = NameField::default();
        update(&mut eq, &mut name, Msg::NameOpen);
        assert!(name.open);
        // Пусто.
        assert_eq!(
            update(&mut eq, &mut name, Msg::SavePreset),
            Effect::default()
        );
        assert_eq!(name.error, Some(PresetError::Empty));
        // Ввод убирает ошибку.
        update(&mut eq, &mut name, Msg::NameInput("  ".to_owned()));
        assert_eq!(name.error, None);
        update(&mut eq, &mut name, Msg::SavePreset);
        assert_eq!(name.error, Some(PresetError::Empty));
        // Повтор встроенного без учёта регистра.
        update(&mut eq, &mut name, Msg::NameInput("рОК".to_owned()));
        update(&mut eq, &mut name, Msg::SavePreset);
        assert_eq!(name.error, Some(PresetError::Duplicate));
        // Слишком длинное.
        update(&mut eq, &mut name, Msg::NameInput("я".repeat(33)));
        update(&mut eq, &mut name, Msg::SavePreset);
        assert_eq!(name.error, Some(PresetError::TooLong));
        assert!(name.open);
        assert!(eq.custom.is_empty());
    }

    #[test]
    fn save_and_remove_custom_preset() {
        let mut eq = EqState::default();
        let mut name = NameField::default();
        run(&mut eq, Msg::Band(1, 25));
        run(&mut eq, Msg::Preamp(-15));
        update(&mut eq, &mut name, Msg::NameOpen);
        update(&mut eq, &mut name, Msg::NameInput("  Наушники ".to_owned()));
        let effect = update(&mut eq, &mut name, Msg::SavePreset);
        assert_eq!(effect, SAVE_ONLY);
        assert_eq!(name, NameField::default());
        let custom = eq.custom.first().cloned();
        assert_eq!(custom.as_ref().map(|c| c.name.as_str()), Some("Наушники"));
        assert_eq!(custom.map(|c| (c.preamp, c.gains[1])), Some((-15, 25)));
        assert_eq!(eq.preset, Some(PresetId::Custom("Наушники".to_owned())));
        // Повтор своего имени без учёта регистра.
        update(&mut eq, &mut name, Msg::NameInput("наушники".to_owned()));
        update(&mut eq, &mut name, Msg::SavePreset);
        assert_eq!(name.error, Some(PresetError::Duplicate));
        // Удаление выбранного снимает выбор, значения остаются.
        let effect = run(&mut eq, Msg::RemovePreset("Наушники".to_owned()));
        assert_eq!(effect, SAVE_ONLY);
        assert!(eq.custom.is_empty());
        assert_eq!(eq.preset, None);
        assert_eq!(eq.settings.gains[1], 25);
    }

    #[test]
    fn removing_other_preset_keeps_selection() {
        let mut eq = EqState::default();
        let mut name = NameField::default();
        for title in ["Один", "Два"] {
            name.text = title.to_owned();
            update(&mut eq, &mut name, Msg::SavePreset);
        }
        assert_eq!(eq.preset, Some(PresetId::Custom("Два".to_owned())));
        run(&mut eq, Msg::RemovePreset("Один".to_owned()));
        assert_eq!(eq.preset, Some(PresetId::Custom("Два".to_owned())));
        assert_eq!(eq.custom.len(), 1);
    }

    #[test]
    fn name_cancel_closes_field() {
        let mut eq = EqState::default();
        let mut name = NameField::default();
        update(&mut eq, &mut name, Msg::NameOpen);
        update(&mut eq, &mut name, Msg::NameInput("abc".to_owned()));
        update(&mut eq, &mut name, Msg::NameCancel);
        assert_eq!(name, NameField::default());
    }

    #[test]
    fn preamp_slider_snaps_to_half_decibel() {
        assert_eq!(preamp_from_slider(-3.0), -30);
        assert_eq!(preamp_from_slider(2.26), 25);
        assert_eq!(preamp_from_slider(-0.2), 0);
        assert_eq!(preamp_from_slider(99.0), GAIN_LIMIT);
        assert_eq!(preamp_from_slider(-99.0), -GAIN_LIMIT);
        assert_eq!(preamp_from_slider(f32::NAN), 0);
    }

    #[test]
    fn selected_title_names_preset() {
        let mut eq = EqState::default();
        assert_eq!(selected_title(&eq), "Свои значения");
        eq.preset = Some(PresetId::Builtin("v-shape"));
        assert_eq!(selected_title(&eq), "Бас и верх");
        eq.preset = Some(PresetId::Custom("Мой".to_owned()));
        assert_eq!(selected_title(&eq), "Мой");
    }
}
