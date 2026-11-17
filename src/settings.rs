use crate::{AppEvent, config::Config, hotkey::Shortcut, tray::NativeBridge};
use std::{cell::RefCell, rc::Rc};
use windui::{
    core::Widget,
    event::{Event, Key, KeyEvent, MouseButton, PointerKind},
    prelude::*,
    render::Canvas,
};

pub fn view(
    config_state: Rc<RefCell<Config>>,
    bridge: Rc<NativeBridge>,
    tx: Sender<AppEvent>,
    status: Signal<String>,
) -> Element {
    let hotkey_input = signal(config_state.borrow().menu_hotkey.clone());
    let middle_input = signal(config_state.borrow().middle_click);
    let tools = crate::tools::registered();
    let fields = Rc::new(
        tools
            .iter()
            .map(|tool| {
                let config = config_state.borrow();
                (
                    tool.id(),
                    signal(config.is_enabled(tool.id())),
                    signal(
                        config
                            .tool_hotkeys
                            .get(tool.id())
                            .cloned()
                            .unwrap_or_default(),
                    ),
                )
            })
            .collect::<Vec<_>>(),
    );
    let save_fields = fields.clone();
    let save = Rc::new(move |ctx: &mut windui::core::EventCtx<'_>| {
        let mut updated = config_state.borrow().clone();
        updated.menu_hotkey = hotkey_input.get().trim().into();
        updated.middle_click = middle_input.get();
        for (id, enabled, shortcut) in save_fields.iter() {
            update_tool_config(&mut updated, id, enabled.get(), &shortcut.get());
        }
        match bridge.apply_config(&updated, true) {
            Ok(()) => {
                *config_state.borrow_mut() = updated;
                status.set("设置已保存并生效".into());
                ctx.toast_ok("已保存");
            }
            Err(error) => {
                let message = format!("设置未应用：{error:#}");
                status.set(message.clone());
                ctx.toast_err("设置未应用，请检查快捷键或配置写入权限");
                let _ = tx.send(AppEvent::Feedback(Err(message)));
            }
        }
    });
    let selected_tab = signal(0usize);
    let general_page = Element::scroll().fill().child(
        Element::col()
            .width_match()
            .padding_xy(2, 14)
            .spacing(12)
            .child(
                Element::label("常规设置")
                    .font_size(22.0)
                    .height(32)
                    .width_match(),
            )
            .child(
                Element::label("设置功能菜单的唤起方式。修改会与工具设置一起保存并立即生效。")
                    .height(42)
                    .width_match()
                    .fg_role(Role::TextMuted),
            )
            .child(
                Element::label("唤起功能菜单的全局热键")
                    .height(24)
                    .width_match(),
            )
            .child(hotkey_capture(hotkey_input, status, false))
            .child(Element::setting_row(
                "全局中键弹出菜单（释放时触发，不吞原点击）",
                Element::switch(middle_input),
            )),
    );

    let selected_tool = signal(0usize);
    let tool_names = tools
        .iter()
        .map(|tool| tool.name().to_string())
        .collect::<Vec<_>>();
    let sidebar = Element::col()
        .width(176)
        .height_match()
        .spacing(8)
        .child(
            Element::label("工具")
                .font_size(18.0)
                .height(28)
                .width_match(),
        )
        .child(
            Element::list_pill(tool_names, selected_tool)
                .width_match()
                .weight(1.0)
                .bg_role(Role::SurfaceAlt)
                .corner(6.0),
        );
    let mut tool_details = Element::stack().fill().weight(1.0);
    for (index, (tool, (_, enabled, shortcut))) in tools.iter().zip(fields.iter()).enumerate() {
        let detail = Element::scroll().fill().child(
            Element::col()
                .width_match()
                .padding_xy(16, 0)
                .spacing(6)
                .child(
                    Element::label(tool.name())
                        .font_size(22.0)
                        .height(32)
                        .width_match(),
                )
                .child(
                    Element::label(tool.description())
                        .height(46)
                        .width_match()
                        .fg_role(Role::TextMuted),
                )
                .child(Element::divider())
                .child(Element::setting_row(
                    "在工具菜单中启用",
                    Element::switch(*enabled),
                ))
                .child(Element::label("全局热键").height(24).width_match())
                .child(hotkey_capture(*shortcut, status, true))
                .child(
                    Element::label("不设置时仍可从托盘菜单运行；Backspace 或 Delete 清除热键。")
                        .height(38)
                        .width_match()
                        .fg_role(Role::TextMuted),
                ),
        );
        tool_details =
            tool_details.child(detail.visible_when(move || selected_tool.get() == index));
    }
    let tools_page = Element::row()
        .fill()
        .padding_xy(2, 14)
        .spacing(14)
        .child(sidebar)
        .child(
            Element::leaf()
                .width(1)
                .height_match()
                .bg_role(Role::Border),
        )
        .child(tool_details);
    let save_all = save.clone();

    Element::col()
        .fill()
        .padding(26)
        .spacing(10)
        .child(
            Element::label("TrayKit")
                .font_size(28.0)
                .height(38)
                .width_match(),
        )
        .child(
            Element::label("托盘工具箱 · 图片转换全程不落盘")
                .fg_role(Role::TextMuted)
                .height(24)
                .width_match(),
        )
        .child(
            Element::tabs_pill(
                selected_tab,
                vec![("常规", general_page), ("工具", tools_page)],
            )
            .weight(1.0),
        )
        .child(Element::divider())
        .child(Element::label_signal(status).height(44).width_match())
        .child(
            Element::row()
                .width_match()
                .height(40)
                .spacing(10)
                .child(
                    Element::button("保存并应用全部设置")
                        .on_click(move |ctx| save_all(ctx))
                        .weight(1.0),
                )
                .child(
                    Element::button("隐藏到托盘")
                        .neutral()
                        .on_click(|ctx| ctx.hide_window()),
                ),
        )
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CaptureAction {
    Pass,
    Pending,
    Cancel,
    Clear,
    Set(String),
    Reject(&'static str),
}

fn capture_action(event: &KeyEvent) -> CaptureAction {
    if !event.pressed {
        return if event.key == Key::Alt {
            CaptureAction::Pending
        } else {
            CaptureAction::Pass
        };
    }
    if event.key == Key::Tab {
        return CaptureAction::Pass;
    }
    if event.key == Key::Escape {
        return CaptureAction::Cancel;
    }
    let has_modifier = event.ctrl || event.alt || event.shift || event.meta;
    if !has_modifier && matches!(event.key, Key::Backspace | Key::Delete) {
        return CaptureAction::Clear;
    }
    if event.key == Key::Alt
        || matches!(
            event.key,
            Key::Other(0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0 | 0xA1 | 0xA2 | 0xA3 | 0xA4 | 0xA5)
        )
    {
        return CaptureAction::Pending;
    }
    // WM_CHAR follows WM_KEYDOWN for printable keys. The first event captures the physical
    // chord; ignoring the translated character prevents a duplicate, modifier-free update.
    if matches!(event.key, Key::Char(_)) {
        return CaptureAction::Pass;
    }
    let key = match event.key {
        Key::Space => 0x20,
        Key::Home => 0x24,
        Key::End => 0x23,
        Key::Insert => 0x2D,
        Key::Delete => 0x2E,
        Key::F(number @ 1..=12) => 0x70 + u32::from(number) - 1,
        Key::Other(key)
            if (b'0' as u32..=b'9' as u32).contains(&key)
                || (b'A' as u32..=b'Z' as u32).contains(&key)
                || (0x7C..=0x87).contains(&key) =>
        {
            key
        }
        _ => return CaptureAction::Reject("该按键暂不支持作为全局热键"),
    };
    if !has_modifier {
        return CaptureAction::Reject("快捷键至少需要 Ctrl、Alt、Shift 或 Win");
    }
    let shortcut = Shortcut {
        modifiers: u32::from(event.alt)
            | (u32::from(event.ctrl) << 1)
            | (u32::from(event.shift) << 2)
            | (u32::from(event.meta) << 3),
        key,
    };
    CaptureAction::Set(shortcut.to_string())
}

struct HotkeyCapture {
    value: Signal<String>,
    status: Signal<String>,
    allow_empty: bool,
}

impl Widget for HotkeyCapture {
    fn paint(
        &self,
        _bounds: Rect,
        content: Rect,
        _focused: bool,
        enabled: bool,
        canvas: &mut dyn Canvas,
        style: &Style,
    ) {
        let value = self.value.get();
        let (text, role) = if value.is_empty() {
            ("点击后按下组合键", Role::Placeholder)
        } else {
            (value.as_str(), Role::Text)
        };
        let theme = windui::theme::current();
        let color = if enabled {
            role.resolve(&theme)
        } else {
            Role::TextDisabled.resolve(&theme)
        };
        canvas.draw_text(
            text,
            content,
            color,
            Align::Start,
            &windui::text::TextStyle::of(style),
        );
    }

    fn on_event(&mut self, ctx: &mut windui::core::EventCtx<'_>, event: &Event) -> bool {
        match event {
            Event::Pointer(pointer)
                if pointer.kind == PointerKind::Down && pointer.button == MouseButton::Left =>
            {
                ctx.request_focus();
                self.status
                    .set("请按下组合键；Esc 取消，Backspace 或 Delete 清除工具热键".into());
                ctx.mark_dirty();
                true
            }
            Event::Key(key) => match capture_action(key) {
                CaptureAction::Pass => false,
                CaptureAction::Pending => {
                    self.status.set("继续按下一个主键".into());
                    true
                }
                CaptureAction::Cancel => {
                    self.status.set("已取消，热键未改变".into());
                    true
                }
                CaptureAction::Clear if self.allow_empty => {
                    self.value.set(String::new());
                    self.status.set("已清除工具热键；保存后生效".into());
                    ctx.mark_dirty();
                    true
                }
                CaptureAction::Clear => {
                    self.status.set("菜单热键不能为空".into());
                    true
                }
                CaptureAction::Set(value) => {
                    self.value.set(value.clone());
                    self.status.set(format!("已捕获 {value}；保存后生效"));
                    ctx.mark_dirty();
                    true
                }
                CaptureAction::Reject(message) => {
                    self.status.set(message.into());
                    true
                }
            },
            _ => false,
        }
    }

    fn focusable(&self) -> bool {
        true
    }
}

fn hotkey_capture(value: Signal<String>, status: Signal<String>, allow_empty: bool) -> Element {
    Element::leaf()
        .widget(HotkeyCapture {
            value,
            status,
            allow_empty,
        })
        .height(40)
        .width_match()
        .padding_xy(10, 8)
        .bg_role(Role::InputBg)
        .border_role(Role::InputBorder, 1)
        .corner(4.0)
}

fn update_tool_config(config: &mut Config, id: &str, enabled: bool, shortcut: &str) {
    if enabled {
        config.disabled_tools.remove(id);
    } else {
        config.disabled_tools.insert(id.into());
    }
    let shortcut = shortcut.trim();
    if shortcut.is_empty() {
        config.tool_hotkeys.remove(id);
    } else {
        config.tool_hotkeys.insert(id.into(), shortcut.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key_event(key: Key, ctrl: bool, alt: bool, shift: bool, meta: bool) -> KeyEvent {
        KeyEvent {
            key,
            pressed: true,
            shift,
            ctrl,
            alt,
            meta,
        }
    }

    #[test]
    fn key_capture_uses_physical_chord_and_ignores_translated_character() {
        assert_eq!(
            capture_action(&key_event(
                Key::Other(b'Q' as u32),
                true,
                true,
                false,
                false
            )),
            CaptureAction::Set("Ctrl+Alt+Q".into())
        );
        assert_eq!(
            capture_action(&key_event(Key::Char('q'), false, false, false, false)),
            CaptureAction::Pass
        );
        assert_eq!(
            capture_action(&key_event(Key::Other(0x11), true, false, false, false)),
            CaptureAction::Pending
        );
    }

    #[test]
    fn key_capture_handles_extended_function_clear_and_invalid_keys() {
        assert_eq!(
            capture_action(&key_event(Key::Other(0x87), true, false, true, false)),
            CaptureAction::Set("Ctrl+Shift+F24".into())
        );
        assert_eq!(
            capture_action(&key_event(Key::Backspace, false, false, false, false)),
            CaptureAction::Clear
        );
        assert_eq!(
            capture_action(&key_event(
                Key::Other(b'K' as u32),
                false,
                false,
                false,
                false
            )),
            CaptureAction::Reject("快捷键至少需要 Ctrl、Alt、Shift 或 Win")
        );
        assert_eq!(
            capture_action(&key_event(Key::Left, true, false, false, false)),
            CaptureAction::Reject("该按键暂不支持作为全局热键")
        );
    }

    #[test]
    fn captured_shortcuts_roundtrip_through_existing_parser() {
        for value in ["Ctrl+Alt+Q", "Ctrl+Shift+F24", "Alt+Win+Delete"] {
            let shortcut: Shortcut = value.parse().unwrap();
            assert_eq!(shortcut.to_string(), value);
        }
    }

    #[test]
    fn empty_shortcut_removes_binding_and_disabled_tool_retains_text() {
        let mut config = Config::default();
        config
            .tool_hotkeys
            .insert("path-rdp".into(), "Ctrl+Alt+F10".into());
        update_tool_config(&mut config, "path-rdp", false, "  Ctrl+Alt+F9  ");
        assert!(!config.is_enabled("path-rdp"));
        assert_eq!(config.tool_hotkeys["path-rdp"], "Ctrl+Alt+F9");
        update_tool_config(&mut config, "path-rdp", true, "   ");
        assert!(config.is_enabled("path-rdp"));
        assert!(!config.tool_hotkeys.contains_key("path-rdp"));
    }
}
