use dioxus::prelude::*;

use super::{ButtonLayout, ButtonTone};

#[derive(Props, Clone, PartialEq)]
pub struct ButtonProps {
    #[props(default)]
    pub class: String,
    #[props(default)]
    pub layout: ButtonLayout,
    #[props(default)]
    pub tone: ButtonTone,
    #[props(default)]
    pub selected: Option<bool>,
    #[props(default)]
    pub disabled: bool,
    #[props(extends = button, extends = GlobalAttributes)]
    pub attributes: Vec<Attribute>,
    pub onclick: EventHandler<MouseEvent>,
    pub children: Element,
}

#[component]
pub fn Button(props: ButtonProps) -> Element {
    let selected = if props.selected == Some(true) {
        "selected"
    } else {
        ""
    };
    rsx! {
        button {
            class: "editor-button {props.layout.class()} {props.tone.class()} {selected} {props.class}",
            r#type: "button",
            disabled: props.disabled.then_some(true),
            onclick:move |event| props.onclick.call(event),
            "aria-pressed": props.selected.map(|value| value.to_string()),
            ..props.attributes,
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct IconButtonProps {
    pub label: String,
    #[props(default)]
    pub class: String,
    #[props(default)]
    pub disabled: bool,
    pub onclick: EventHandler<MouseEvent>,
    pub children: Element,
}

#[component]
pub fn IconButton(props: IconButtonProps) -> Element {
    rsx! {
        Button {
            class: "editor-icon-button {props.class}",
            layout: ButtonLayout::Icon,
            disabled: props.disabled,
            onclick: move |event| props.onclick.call(event),
            "aria-label": props.label,
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct StatusBadgeProps {
    pub label: String,
    #[props(default)]
    pub tone: String,
}

#[component]
pub fn StatusBadge(props: StatusBadgeProps) -> Element {
    rsx! { span { class: "editor-status-badge {props.tone}", "{props.label}" } }
}

#[derive(Props, Clone, PartialEq)]
pub struct ToggleGroupProps {
    #[props(default)]
    pub class: String,
    pub label: String,
    pub children: Element,
}

#[component]
pub fn ToggleGroup(props: ToggleGroupProps) -> Element {
    rsx! { div { class: "editor-toggle-group {props.class}", role: "group", "aria-label": props.label, {props.children} } }
}

#[derive(Props, Clone, PartialEq)]
pub struct SegmentedControlProps {
    #[props(default)]
    pub class: String,
    pub label: String,
    pub children: Element,
}

#[component]
pub fn SegmentedControl(props: SegmentedControlProps) -> Element {
    rsx! { div { class: "editor-segmented-control {props.class}", role: "group", "aria-label": props.label, {props.children} } }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListPickerOption {
    pub label: String,
    pub detail: Option<String>,
}

#[derive(Props, Clone, PartialEq)]
pub struct ListPickerProps {
    pub label: String,
    pub value: String,
    pub options: Vec<ListPickerOption>,
    pub selected: usize,
    pub open: bool,
    #[props(default)]
    pub disabled: bool,
    #[props(default)]
    pub class: String,
    pub onopen: EventHandler<bool>,
    pub onselect: EventHandler<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListPickerKeyAction {
    Open(usize),
    Move(usize),
    Select(usize),
    Close,
}

fn list_picker_key_action(
    key: &Key,
    open: bool,
    cursor: usize,
    selected: usize,
    option_count: usize,
) -> Option<ListPickerKeyAction> {
    match key {
        Key::ArrowDown | Key::ArrowRight if option_count > 0 => Some(if open {
            ListPickerKeyAction::Move((cursor + 1) % option_count)
        } else {
            ListPickerKeyAction::Open(selected.min(option_count - 1))
        }),
        Key::ArrowUp | Key::ArrowLeft if option_count > 0 => Some(if open {
            ListPickerKeyAction::Move((cursor + option_count - 1) % option_count)
        } else {
            ListPickerKeyAction::Open(selected.min(option_count - 1))
        }),
        Key::Home if open && option_count > 0 => Some(ListPickerKeyAction::Move(0)),
        Key::End if open && option_count > 0 => Some(ListPickerKeyAction::Move(option_count - 1)),
        Key::Enter if open && option_count > 0 => {
            Some(ListPickerKeyAction::Select(cursor.min(option_count - 1)))
        }
        Key::Character(value) if value == " " && open && option_count > 0 => {
            Some(ListPickerKeyAction::Select(cursor.min(option_count - 1)))
        }
        Key::Escape if open => Some(ListPickerKeyAction::Close),
        _ => None,
    }
}

#[component]
pub fn ListPicker(props: ListPickerProps) -> Element {
    let mut cursor = use_signal(|| props.selected);
    let option_count = props.options.len();
    let keyboard_props = props.clone();
    let accessible_label = format!("{}: {}", props.label, props.value);
    rsx! {
        div {
            class: "editor-list-picker {props.class}",
            onkeydown: move |event| {
                if let Some(action) = list_picker_key_action(
                    &event.key(),
                    keyboard_props.open,
                    cursor(),
                    keyboard_props.selected,
                    option_count,
                ) {
                    match action {
                        ListPickerKeyAction::Open(index) => {
                            cursor.set(index);
                            keyboard_props.onopen.call(true);
                        }
                        ListPickerKeyAction::Move(index) => cursor.set(index),
                        ListPickerKeyAction::Select(index) => {
                            keyboard_props.onopen.call(false);
                            keyboard_props.onselect.call(index);
                        }
                        ListPickerKeyAction::Close => keyboard_props.onopen.call(false),
                    }
                    event.prevent_default();
                    event.stop_propagation();
                }
            },
            Button {
                class: "list-picker-trigger",
                disabled: props.disabled,
                onclick: move |_| {
                    cursor.set(props.selected);
                    props.onopen.call(!props.open);
                },
                "aria-label": accessible_label,
                "aria-expanded": props.open.to_string(),
                "aria-haspopup": "listbox",
                span { class: "list-picker-label", "{props.label}" }
                b { "{props.value}" }
                span { aria_hidden: "true", if props.open { "−" } else { "+" } }
            }
            if props.open {
                div { class: "list-picker-options", role: "listbox", "aria-label": props.label,
                    for (index, option) in props.options.iter().enumerate() {
                        Button {
                            class: if cursor() == index { "list-picker-option cursor" } else { "list-picker-option" },
                            layout: ButtonLayout::Row,
                            selected: props.selected == index,
                            onclick: move |_| {
                                props.onopen.call(false);
                                props.onselect.call(index);
                            },
                            role: "option",
                            "data-index": index,
                            "aria-selected": (props.selected == index).to_string(),
                            span { "{option.label}" }
                            if let Some(detail) = &option.detail { small { "{detail}" } }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ListPickerKeyAction, list_picker_key_action};
    use dioxus::prelude::Key;

    #[test]
    fn list_picker_keyboard_contract_wraps_selects_and_closes() {
        assert_eq!(
            list_picker_key_action(&Key::ArrowDown, false, 0, 2, 4),
            Some(ListPickerKeyAction::Open(2))
        );
        assert_eq!(
            list_picker_key_action(&Key::ArrowDown, true, 3, 2, 4),
            Some(ListPickerKeyAction::Move(0))
        );
        assert_eq!(
            list_picker_key_action(&Key::ArrowUp, true, 0, 2, 4),
            Some(ListPickerKeyAction::Move(3))
        );
        assert_eq!(
            list_picker_key_action(&Key::Home, true, 2, 2, 4),
            Some(ListPickerKeyAction::Move(0))
        );
        assert_eq!(
            list_picker_key_action(&Key::End, true, 2, 2, 4),
            Some(ListPickerKeyAction::Move(3))
        );
        assert_eq!(
            list_picker_key_action(&Key::Enter, true, 3, 2, 4),
            Some(ListPickerKeyAction::Select(3))
        );
        assert_eq!(
            list_picker_key_action(&Key::Character(" ".into()), true, 1, 2, 4),
            Some(ListPickerKeyAction::Select(1))
        );
        assert_eq!(
            list_picker_key_action(&Key::Escape, true, 1, 2, 4),
            Some(ListPickerKeyAction::Close)
        );
        assert_eq!(list_picker_key_action(&Key::Escape, false, 1, 2, 4), None);
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct NavigatorTreeProps {
    pub label: String,
    pub children: Element,
}

#[component]
pub fn NavigatorTree(props: NavigatorTreeProps) -> Element {
    rsx! { div { class: "navigator-tree", role: "tree", "aria-label": props.label, {props.children} } }
}

#[derive(Props, Clone, PartialEq)]
pub struct NavigatorItemProps {
    pub label: String,
    #[props(default)]
    pub detail: Option<String>,
    #[props(default)]
    pub class: String,
    #[props(default)]
    pub depth: usize,
    #[props(default)]
    pub selected: bool,
    #[props(default)]
    pub expanded: Option<bool>,
    pub onclick: EventHandler<MouseEvent>,
    #[props(default)]
    pub ontoggle: Option<EventHandler<MouseEvent>>,
    #[props(extends = GlobalAttributes)]
    pub attributes: Vec<Attribute>,
    #[props(default)]
    pub children: Element,
}

#[component]
pub fn NavigatorItem(props: NavigatorItemProps) -> Element {
    let expanded = props.expanded;
    rsx! {
        div { class: "navigator-item {props.class}", role: "treeitem", "aria-expanded": expanded.map(|value| value.to_string()), ..props.attributes,
            div { class: "navigator-item-line", "data-depth": props.depth,
                if let Some(handler) = props.ontoggle {
                    IconButton { class: "tree-disclosure", label: format!("Toggle {} children", props.label), onclick: move |event| handler.call(event), if expanded == Some(true) { "−" } else { "+" } }
                } else { span { class: "tree-spacer" } }
                Button { class: "navigator-item-select", layout: ButtonLayout::Row, selected: props.selected, onclick: move |event| props.onclick.call(event), span { "{props.label}" } if let Some(detail) = &props.detail { small { "{detail}" } } }
            }
            if expanded != Some(false) { div { class: "navigator-children", role: "group", {props.children} } }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct AccordionProps {
    pub label: String,
    pub children: Element,
}

#[component]
pub fn Accordion(props: AccordionProps) -> Element {
    rsx! { div { class: "editor-accordion-group", "aria-label": props.label, {props.children} } }
}

#[derive(Props, Clone, PartialEq)]
pub struct TextFieldProps {
    pub field_key: String,
    pub label: String,
    pub value: String,
    #[props(default)]
    pub disallowed: Vec<String>,
    #[props(default)]
    pub disabled: bool,
    #[props(default)]
    pub update_on_input: bool,
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
                    if input_props.update_on_input {
                        input_props.onchange.call(text());
                    }
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
                    if valid && !blur_props.update_on_input && value != blur_props.value {
                        blur_props.onchange.call(value);
                    }
                },
                onkeydown: move |event| if event.key() == Key::Enter {
                    event.prevent_default();
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
        Button {
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
