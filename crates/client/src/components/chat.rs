use std::time::Duration;

use color_eyre::eyre::Result;
use common::protocol;
use ratatui::{
    Frame,
    crossterm::event::{self, KeyEvent},
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};
use tokio::sync::mpsc::UnboundedSender;
use tui_input::backend::crossterm::EventHandler;
use websocket::message::Message;

use crate::{AppEvent, EventSender, component::Component, components::Urgency, into_ratatui_color};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
}

#[derive(Debug)]
pub struct Chat<'a> {
    mode: Mode,
    token: Option<protocol::Token>,

    received_messages: Vec<Line<'a>>,
    /// If `None`, snap to the bottom. Otherwise, fixed scroll towards the top.
    chat_scroll_neg: Option<usize>,
    current_input: tui_input::Input,
    input_scroll: usize,

    ws_tx: UnboundedSender<Message>,
    event_tx: EventSender,
}

struct ChatWidget<'a> {
    messages: &'a [Line<'a>],
    scroll_neg: &'a mut Option<usize>,
    authorized: bool,
}

impl<'a> ChatWidget<'a> {
    fn clamp_scroll(&mut self, area: &Rect, text_height: usize) -> usize {
        let view_height = area.height.saturating_sub(2) as usize;

        *self.scroll_neg = self
            .scroll_neg
            .map(|scroll| scroll.min(text_height.saturating_sub(view_height)));

        let mut scroll = text_height.saturating_sub(view_height + self.scroll_neg.unwrap_or(0));
        if let Some(0) = self.scroll_neg {
            *self.scroll_neg = None;
        }
        if self.scroll_neg.is_none() && text_height - scroll > view_height {
            scroll = view_height.saturating_add(area.height as usize);
        }
        scroll
    }
}

impl<'a> Widget for ChatWidget<'a> {
    fn render(mut self, area: Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let mut chat_block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title_top(
                (Span::raw(" j↓  k↑").bold().green() + Span::raw(" to scroll ")).right_aligned(),
            )
            .title_top((Span::raw(" q").bold().green() + Span::raw(" to quit ")).left_aligned());
        if !self.authorized {
            chat_block = chat_block.title_top(
                Span::raw(" Authenticate first! ")
                    .red()
                    .into_centered_line(),
            );
        }

        let mut chat_paragraph = Paragraph::new(self.messages.to_vec())
            .block(chat_block.clone())
            .wrap(ratatui::widgets::Wrap { trim: false });
        let line_count = chat_paragraph.line_count(area.width).saturating_sub(2);
        chat_paragraph = chat_paragraph.scroll((self.clamp_scroll(&area, line_count) as u16, 0));
        chat_paragraph.render(area, buf);
    }
}

struct InputWidget<'a> {
    input: &'a tui_input::Input,
    mode: Mode,
    scroll: &'a mut usize,
}

impl<'a> InputWidget<'a> {
    fn cursor_position(&mut self, area: Rect) -> (u16, u16) {
        let width = area.width as usize - 2;
        let height = area.height as usize - 2;
        let cursor_absolute = self.input.visual_cursor();
        let (cursor_x, mut cursor_y) = (
            cursor_absolute % width,
            cursor_absolute.checked_div(width).unwrap_or(0),
        );
        if cursor_y > (height - 1) {
            *self.scroll = cursor_y - (height - 1);
            cursor_y = height - 1;
        } else {
            *self.scroll = 0;
        }
        (cursor_x as u16 + area.x + 1, cursor_y as u16 + area.y + 1)
    }
}

impl<'a> Widget for InputWidget<'a> {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let input_block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title_top(if self.mode == Mode::Normal {
                Span::raw(" a/i").bold().green() + Span::raw(" to enter INSERT mode ")
            } else {
                Span::raw(" <ESC>").bold().green() + Span::raw(" to exit INSERT mode ")
            })
            .title_alignment(ratatui::layout::Alignment::Right);
        let input_paragraph = Paragraph::new(self.input.value())
            .block(if self.mode == Mode::Insert {
                input_block.blue()
            } else {
                input_block
            })
            .wrap(ratatui::widgets::Wrap { trim: false })
            .scroll((*self.scroll as u16, 0));
        input_paragraph.render(area, buf);
    }
}

impl Chat<'_> {
    pub fn new(ws_tx: UnboundedSender<Message>, event_tx: EventSender) -> Box<Self> {
        Box::new(Self {
            mode: Mode::default(),
            token: None,
            received_messages: vec![],
            chat_scroll_neg: None,
            current_input: tui_input::Input::default(),
            input_scroll: 0,
            ws_tx,
            event_tx,
        })
    }

    async fn handle_key_event(&mut self, event: KeyEvent) -> Result<bool> {
        Ok(match self.mode {
            Mode::Normal => match event.code {
                event::KeyCode::Char('i' | 'a') => {
                    self.mode = Mode::Insert;
                    true
                }
                event::KeyCode::Char('j') => {
                    self.chat_scroll_neg =
                        Some(self.chat_scroll_neg.unwrap_or(0).saturating_sub(1));
                    true
                }
                event::KeyCode::Char('k') => {
                    self.chat_scroll_neg =
                        Some(self.chat_scroll_neg.unwrap_or(0).saturating_add(1));
                    true
                }
                _ => false,
            },
            Mode::Insert => match event.code {
                event::KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    true
                }
                event::KeyCode::Enter => {
                    self.send_chat_message().await?;
                    true
                }
                _ => self
                    .current_input
                    .handle_event(&event::Event::Key(event))
                    .is_some(),
            },
        })
    }

    async fn handle_ws_message(&mut self, message: Message) -> Result<bool> {
        if let Ok(server_msg) = protocol::ServerMessage::try_from(&message) {
            match server_msg {
                protocol::ServerMessage::AuthSuccess(Err(e)) => {
                    self.event_tx.notify(
                        match e {
                            protocol::AuthError::NicknameUnavailable => {
                                "This nickname is unavailable. Try again."
                            }
                            protocol::AuthError::NicknameTooLong => {
                                "This nickname is too long. Try again."
                            }
                            protocol::AuthError::AlreadyAuthorized => "You are already authorized.",
                        },
                        Urgency::Warning,
                        Duration::from_secs(3),
                    )?;
                    self.event_tx.send(AppEvent::SpawnAuth)?;
                }
                protocol::ServerMessage::AuthSuccess(Ok(token)) => {
                    self.token = Some(token);
                }
                protocol::ServerMessage::PropagateMessage(sender, text) => {
                    self.received_messages.push(
                        Span::styled(
                            sender.name,
                            Style::new().fg(into_ratatui_color(sender.color)),
                        ) + Span::raw(": ")
                            + Span::raw(text),
                    );
                }
                protocol::ServerMessage::Notification(notif) => match notif {
                    protocol::ServerNotification::Literal(text) => {
                        self.event_tx.notify(
                            String::from("Server: ") + &text,
                            Urgency::Info,
                            Duration::from_secs(5),
                        )?;
                    }
                    protocol::ServerNotification::ClientConnected(sender) => {
                        self.received_messages.push(
                            Span::styled(sender.name, into_ratatui_color(sender.color))
                                + Span::raw(" has connected.").gray().italic(),
                        );
                    }
                    protocol::ServerNotification::ClientDisconnected(sender) => {
                        self.received_messages.push(
                            Span::styled(sender.name, into_ratatui_color(sender.color))
                                + Span::raw(" has disconnected.").gray().italic(),
                        );
                    }
                },
                _ => {}
            }
        } else {
            self.received_messages
                .push(Line::from(format!("Couln't parse message: {message:?}")));
        }
        Ok(true)
    }

    async fn send_chat_message(&mut self) -> Result<()> {
        if self.token.is_none() {
            return Ok(());
        }

        self.ws_tx.send(
            protocol::ClientMessage::SendMessage {
                token: self.token.clone().unwrap(),
                text: self.current_input.to_string(),
            }
            .into(),
        )?;
        self.current_input.reset();
        Ok(())
    }
}

#[async_trait::async_trait]
impl Component for Chat<'_> {
    async fn init(&mut self) -> Result<()> {
        self.event_tx.send(AppEvent::SpawnAuth)?;
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _is_focused: bool) {
        let layout = Layout::vertical([Constraint::Fill(1), Constraint::Max(5)]);
        let [chat_area, input_area] = layout.areas(area);

        let chat_widget = ChatWidget {
            messages: &self.received_messages,
            scroll_neg: &mut self.chat_scroll_neg,
            authorized: self.token.is_some(),
        };
        // Mutates the outer state. In my defence,
        // that specific part is determined during rendering.
        chat_widget.render(chat_area, frame.buffer_mut());

        let mut input_widget = InputWidget {
            input: &self.current_input,
            mode: self.mode,
            scroll: &mut self.input_scroll,
        };
        if self.mode == Mode::Insert {
            frame.set_cursor_position(input_widget.cursor_position(input_area));
        }
        input_widget.render(input_area, frame.buffer_mut());
    }

    async fn handle_event(&mut self, event: AppEvent, is_focused: bool) -> Result<bool> {
        Ok(match event {
            AppEvent::KeyEvent(key_event) if is_focused => self.handle_key_event(key_event).await?,
            AppEvent::WsMessage(msg) => self.handle_ws_message(msg).await?,
            _ => false,
        })
    }
}
