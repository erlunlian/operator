use gpui::*;

use crate::settings::AppSettings;
use crate::theme::colors;

actions!(settings_panel, [CloseSettingsWindow]);

pub struct SettingsPanel {
    focus_handle: FocusHandle,
}

impl SettingsPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }

    fn close_window(&mut self, _: &CloseSettingsWindow, window: &mut Window, _cx: &mut Context<Self>) {
        window.remove_window();
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let vim_enabled = AppSettings::vim_mode(cx);
        let entity = cx.entity().clone();

        div()
            .id("settings-panel")
            .flex()
            .flex_col()
            .size_full()
            .bg(colors::surface())
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::close_window))
            .key_context("SettingsPanel")
            // Header
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(colors::border())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors::text())
                            .child("Settings"),
                    ),
            )
            // Settings list
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p_4()
                    .gap_4()
                    // Vim Mode toggle
                    .child(Self::render_toggle(
                        "Vim Mode",
                        "Use vim keybindings in the editor (hjkl, i/a to insert, Esc for normal mode)",
                        vim_enabled,
                        {
                            let entity = entity.clone();
                            move |_window, cx| {
                                let _ = entity; // keep alive
                                cx.update_global::<AppSettings, _>(|settings, _cx| {
                                    settings.vim_mode = !settings.vim_mode;
                                });
                            }
                        },
                    )),
            )
    }
}

impl SettingsPanel {
    fn render_toggle(
        label: &str,
        description: &str,
        enabled: bool,
        on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Div {
        let toggle_bg = if enabled {
            colors::accent()
        } else {
            colors::surface_hover()
        };
        let knob_left = if enabled { px(18.0) } else { px(2.0) };

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(colors::text())
                            .font_weight(FontWeight::MEDIUM)
                            .child(label.to_string()),
                    )
                    .child(
                        // Toggle switch
                        div()
                            .id(ElementId::Name(format!("toggle-{}", label).into()))
                            .w(px(36.0))
                            .h(px(20.0))
                            .rounded(px(10.0))
                            .bg(toggle_bg)
                            .cursor_pointer()
                            .relative()
                            .child(
                                div()
                                    .absolute()
                                    .top(px(2.0))
                                    .left(knob_left)
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .rounded(px(8.0))
                                    .bg(gpui::white()),
                            )
                            .on_click(move |_, window, cx| {
                                on_toggle(window, cx);
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors::text_muted())
                    .child(description.to_string()),
            )
    }
}
