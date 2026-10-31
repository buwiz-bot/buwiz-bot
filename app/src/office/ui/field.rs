//! The dialogs' fields: a labelled row (DR `PageSetupDialog`'s `g`/`c`),
//! a section label (`H`), a text field and a number field (`y`), a
//! checkbox (`FindReplaceDialog`'s `le`/`z`) and tabs (`HyperlinkDialog`'s
//! `ne`/`H`/`F`). Fields inside a [`super::modal`] join its Tab order.

use nsh_core::canvas::{FillRule, svg_path};
use nsh_core::prelude::*;

use super::modal::join_trap;
use super::palette as p;

/// A form row (`g`, `c`): the label 80 wide (13px muted), 12 before the
/// field.
pub(crate) const LABEL_W: f32 = 80.0;
pub(crate) const ROW_GAP: f32 = 12.0;
/// A field (`y`): `padding 6px 8px`, 1px `--doc-border`, radius 4, 13px.
pub(crate) const FIELD_PAD: (f32, f32) = (8.0, 6.0);
pub(crate) const FIELD_RADIUS: f32 = 4.0;
pub(crate) const FIELD_TEXT: f32 = 13.0;
/// A checkbox (`le`): 14 square, 6 before its 12px label.
pub(crate) const CHECKBOX: f32 = 14.0;
/// A tab (`H`): `padding 10px 16px`, 14px, a 2px underline.
pub(crate) const TAB_PAD: (f32, f32) = (16.0, 10.0);

/// `label` then `field` in a row.
pub(crate) fn form_row(label: impl Into<SharedString>, field: impl IntoElement) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(ROW_GAP))
        .child(text(label.into()).w(px(LABEL_W)).text_size(px(13.0)).text_color(p::TEXT_MUTED))
        .child(view().flex_1().child(field))
        .into_element()
}

/// A section's label (`H`): 12px semibold muted capitals, 0.5 apart.
pub(crate) fn section_label(t: &str) -> AnyElement {
    text(t.to_uppercase())
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .letter_spacing(px(0.5))
        .text_color(p::TEXT_MUTED)
        .into_element()
}

fn field_look(i: nsh_core::element::Input) -> nsh_core::element::Input {
    i.w_full()
        .px(px(FIELD_PAD.0))
        .py(px(FIELD_PAD.1))
        .border_1()
        .border_color(p::BORDER)
        .rounded(px(FIELD_RADIUS))
        .text_size(px(FIELD_TEXT))
        .text_color(p::TEXT)
        .placeholder_color(|| color(p::TEXT_PLACEHOLDER))
}

/// A text field holding `value`, labelled `label` for screen readers.
pub(crate) fn text_field(label: impl Into<SharedString>, value: Signal<String>) -> AnyElement {
    let r = element_ref();
    join_trap(&r);
    field_look(nsh_base::input())
        .element_ref(&r)
        .label(label.into())
        .value(move || value.get())
        .on_change(move |s: &str| value.set(s.to_owned()))
        .into_element()
}

/// A number field (`type="number"`): `value` follows what parses, kept in
/// `min`–`max`; the text stays as typed.
pub(crate) fn number_field(label: impl Into<SharedString>, value: Signal<f64>, min: f64, max: f64) -> AnyElement {
    let r = element_ref();
    join_trap(&r);
    let typed = signal(fmt_number(value.get_untracked()));
    field_look(nsh_base::input())
        .element_ref(&r)
        .label(label.into())
        .keyboard_type(KeyboardType::Decimal)
        .value(move || typed.get())
        .on_change(move |s: &str| {
            typed.set(s.to_owned());
            if let Some(n) = parse_number(s, min, max) {
                value.set(n)
            }
        })
        .into_element()
}

/// A number as a field shows it: "1", "0.75".
pub(crate) fn fmt_number(n: f64) -> String {
    if n.fract() == 0.0 { format!("{}", n as i64) } else { format!("{n}") }
}

/// What a number field's text holds, within `min`–`max`.
pub(crate) fn parse_number(s: &str, min: f64, max: f64) -> Option<f64> {
    let n: f64 = s.trim().parse().ok()?;
    n.is_finite().then(|| n.clamp(min, max))
}

const CHECK_D: &str = "M16.704 4.153a.75.75 0 01.143 1.052l-8 10.5a.75.75 0 01-1.127.075l-4.5-4.5a.75.75 0 011.06-1.06l3.894 3.893 7.48-9.817a.75.75 0 011.05-.143z";

/// A checkbox and its label (`z`: 12px muted, 6 apart). The web's is the
/// browser's own control at 14 square; this draws macOS's: a rounded box,
/// filled with the system blue and a white tick while on.
pub(crate) fn checkbox(label: impl Into<SharedString>, on: Signal<bool>) -> AnyElement {
    let r = element_ref();
    join_trap(&r);
    let label: SharedString = label.into();
    let tick = svg_path(CHECK_D);
    let blue = AdaptiveColor { light: hsla(0.5817, 1.0, 0.5, 1.0), dark: hsla(0.5817, 1.0, 0.52, 1.0) };
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .element_ref(&r)
        .label(label.clone())
        .role(AccessibilityRole::Checkbox)
        .on_press(move || on.update(|v| *v = !*v))
        .child(
            view()
                .size(px(CHECKBOX))
                .rounded(px(3.0))
                .border(move || px(if on.get() { 0.0 } else { 1.0 }))
                .border_color(AdaptiveColor { light: hsla(0.0, 0.0, 0.0, 0.25), dark: hsla(0.0, 0.0, 1.0, 0.3) })
                .bg(move || if on.get() { blue } else { p::SURFACE })
                .child(when(
                    move || on.get(),
                    move || {
                        let tick = tick.clone();
                        canvas(move || Painter::new().translate(px(2.0), px(2.0)).scale(0.5, 0.5).fill_color(white()).fill_path(&tick, FillRule::EvenOdd))
                            .size(px(CHECKBOX))
                    },
                )),
        )
        .child(text(label).text_size(px(12.0)).text_color(p::TEXT_MUTED))
        .into_element()
}

/// Tabs (`ne`): a row over a hairline, each tab muted with a clear
/// underline, the chosen one `--doc-link` and medium with its underline
/// in that colour; `current` is the chosen tab's index.
pub(crate) fn tabs(names: Vec<&'static str>, current: Signal<usize>) -> AnyElement {
    view()
        .flex_row()
        .border_b(px(1.0))
        .border_color(p::BORDER)
        .mb(px(16.0))
        .label("Tabs")
        .children(names.into_iter().enumerate().map(|(i, name)| {
            // a tab's weight is no reactive style: it is drawn again as the
            // choice changes
            dynamic(move || {
                let on = current.get() == i;
                text(name)
                    .px(px(TAB_PAD.0))
                    .py(px(TAB_PAD.1))
                    .mb(px(-1.0))
                    .border_b(px(2.0))
                    .border_color(if on { p::LINK } else { AdaptiveColor::TRANSPARENT })
                    .text_size(px(14.0))
                    .font_weight(if on { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                    .text_color(if on { p::LINK } else { p::TEXT_MUTED })
                    .label(name)
                    .role(AccessibilityRole::Tab)
                    .on_press(move || current.set(i))
                    .into_element()
            })
            .into_element()
        }))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_fields_parse_within_their_limits() {
        assert_eq!(parse_number("1.5", 0.0, 10.0), Some(1.5));
        assert_eq!(parse_number("12", 0.0, 10.0), Some(10.0));
        assert_eq!(parse_number("x", 0.0, 10.0), None);
        assert_eq!(fmt_number(1.0), "1");
        assert_eq!(fmt_number(0.75), "0.75");
    }
}
