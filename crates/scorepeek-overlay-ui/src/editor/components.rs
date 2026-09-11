use dioxus::prelude::*;

use super::{ButtonLayout, EditorButton};

#[derive(Props, Clone, PartialEq)]
pub struct TextFieldProps {
    pub field_key: String,
    pub label: String,
    pub value: String,
    #[props(default)]
    pub disallowed: Vec<String>,
    #[props(default)]
    pub disabled: bool,
    pub onchange: EventHandler<String>,
    pub onvalidity: EventHandler<(String, bool)>,
}

#[component]
pub fn TextField(props: TextFieldProps) -> Element {
    let mut text = use_signal(|| props.value.clone());
    let mut focused = use_signal(|| false);
    let mut invalid = use_signal(|| false);
    let cleanup_props = props.clone();
    use_drop(move || {
        cleanup_props
            .onvalidity
            .call((cleanup_props.field_key.clone(), true));
    });
    let displayed = if focused() || invalid() {
        text()
    } else {
        props.value.clone()
    };
    let error = if displayed.trim().is_empty() {
        Some("Name is required")
    } else if props.disallowed.contains(&displayed) {
        Some("Name must be unique")
    } else {
        None
    };
    let focus_props = props.clone();
    let input_props = props.clone();
    let blur_props = props.clone();
    rsx! {
        label { class: if error.is_some() { "editor-field invalid" } else { "editor-field" },
            span { class: "editor-field-label", "{props.label}" }
            input {
                class: "editor-text-field",
                r#type: "text",
                value: "{displayed}",
                disabled: props.disabled.then_some(true),
                onfocus: move |_| {
                    if !invalid() {
                        text.set(focus_props.value.clone());
                    }
                    focused.set(true);
                },
                oninput: move |event| {
                    text.set(event.value());
                    let valid = !text().trim().is_empty()
                        && !input_props.disallowed.contains(&text());
                    invalid.set(!valid);
                    input_props
                        .onvalidity
                        .call((input_props.field_key.clone(), valid));
                },
                onblur: move |_| {
                    let value = text();
                    let valid = !value.trim().is_empty()
                        && !blur_props.disallowed.contains(&value);
                    focused.set(false);
                    invalid.set(!valid);
                    blur_props
                        .onvalidity
                        .call((blur_props.field_key.clone(), valid));
                    if valid && value != blur_props.value {
                        blur_props.onchange.call(value);
                    }
                },
                "aria-invalid": error.is_some().then_some("true"),
            }
            if let Some(error) = error { small { role: "alert", "{error}" } }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct NumberFieldProps {
    pub field_key: String,
    pub label: String,
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    #[props(default = 4)]
    pub step: i32,
    #[props(default)]
    pub allow_maximum_off_grid: bool,
    #[props(default)]
    pub disabled: bool,
    pub onchange: EventHandler<i32>,
    pub onvalidity: EventHandler<(String, bool)>,
}

#[component]
pub fn NumberField(props: NumberFieldProps) -> Element {
    let mut text = use_signal(|| props.value.to_string());
    let mut focused = use_signal(|| false);
    let mut invalid = use_signal(|| false);
    let cleanup_props = props.clone();
    use_drop(move || {
        cleanup_props
            .onvalidity
            .call((cleanup_props.field_key.clone(), true));
    });
    let displayed = if focused() || invalid() {
        text()
    } else {
        props.value.to_string()
    };
    let valid = displayed.parse::<i32>().ok().is_some_and(|value| {
        props.minimum <= value
            && value <= props.maximum
            && ((value - props.minimum).rem_euclid(props.step) == 0
                || props.allow_maximum_off_grid && value == props.maximum)
    });
    let commit_blur = {
        let props = props.clone();
        move || {
            let parsed = text().parse::<i32>().ok().filter(|value| {
                props.minimum <= *value
                    && *value <= props.maximum
                    && ((*value - props.minimum).rem_euclid(props.step) == 0
                        || props.allow_maximum_off_grid && *value == props.maximum)
            });
            props
                .onvalidity
                .call((props.field_key.clone(), parsed.is_some()));
            if let Some(value) = parsed {
                props.onchange.call(value);
            }
        }
    };
    let commit_key = {
        let props = props.clone();
        move || {
            let parsed = text().parse::<i32>().ok().filter(|value| {
                props.minimum <= *value
                    && *value <= props.maximum
                    && ((*value - props.minimum).rem_euclid(props.step) == 0
                        || props.allow_maximum_off_grid && *value == props.maximum)
            });
            props
                .onvalidity
                .call((props.field_key.clone(), parsed.is_some()));
            if let Some(value) = parsed {
                props.onchange.call(value);
            }
        }
    };
    let focus_props = props.clone();
    let input_props = props.clone();
    rsx! {
        label { class: if valid { "editor-field number" } else { "editor-field number invalid" },
            span { class: "editor-field-label", "{props.label}" }
            input {
                class: "editor-number-field",
                r#type: "text",
                inputmode: "numeric",
                value: "{displayed}",
                disabled: props.disabled.then_some(true),
                "aria-invalid": (!valid).then_some("true"),
                onfocus: move |_| {
                    if !invalid() {
                        text.set(focus_props.value.to_string());
                    }
                    focused.set(true);
                },
                oninput: move |event| {
                    text.set(event.value());
                    let parsed = text().parse::<i32>().ok().filter(|value| {
                        input_props.minimum <= *value
                            && *value <= input_props.maximum
                            && ((*value - input_props.minimum).rem_euclid(input_props.step) == 0
                                || input_props.allow_maximum_off_grid
                                    && *value == input_props.maximum)
                    });
                    input_props
                        .onvalidity
                        .call((input_props.field_key.clone(), parsed.is_some()));
                    invalid.set(parsed.is_none());
                },
                onblur: move |_| {
                    commit_blur();
                    invalid.set(!valid);
                    focused.set(false);
                },
                onkeydown: move |event| if event.key() == Key::Enter { commit_key() },
            }
            if !valid { small { role: "alert", "{props.minimum}–{props.maximum}, {props.step}px grid or output edge" } }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ToggleProps {
    #[props(default)]
    pub class: String,
    pub label: String,
    pub selected: bool,
    #[props(default)]
    pub disabled: bool,
    #[props(default)]
    pub data_index: Option<usize>,
    #[props(default)]
    pub data_canvas_id: Option<String>,
    pub onclick: EventHandler<MouseEvent>,
}

#[component]
pub fn Toggle(props: ToggleProps) -> Element {
    rsx! {
        EditorButton {
            class: "editor-toggle {props.class}",
            layout: ButtonLayout::Row,
            selected: props.selected,
            disabled: props.disabled,
            onclick: move |event| props.onclick.call(event),
            "data-index": props.data_index,
            "data-canvas-id": props.data_canvas_id,
            span { "{props.label}" }
            b { if props.selected { "ON" } else { "OFF" } }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct AccordionSectionProps {
    pub title: String,
    #[props(default = true)]
    pub initially_open: bool,
    pub children: Element,
}

#[component]
pub fn AccordionSection(props: AccordionSectionProps) -> Element {
    let mut open = use_signal(|| props.initially_open);
    rsx! {
        section { class: "editor-accordion",
            button {
                class: "editor-accordion-heading",
                r#type: "button",
                "aria-expanded": open().to_string(),
                onclick: move |_| open.toggle(),
                span { "{props.title}" }
                i { aria_hidden: "true", if open() { "−" } else { "+" } }
            }
            div { class: "editor-accordion-body", style: if open() { "" } else { "display:none" }, {props.children} }
        }
    }
}
