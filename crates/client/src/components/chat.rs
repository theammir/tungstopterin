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

use crate::{AppEvent, component::Component, into_ratatui_color};

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
    current_input: tui_input::Input,
    received_messages: Vec<Line<'a>>,
    ws_tx: UnboundedSender<Message>,
    event_tx: UnboundedSender<AppEvent>,
}

impl Chat<'_> {
    pub fn new(ws_tx: UnboundedSender<Message>, event_tx: UnboundedSender<AppEvent>) -> Box<Self> {
        Box::new(Self {
            mode: Mode::default(),
            token: None,
            current_input: tui_input::Input::new(String::new()),
            received_messages: vec![],
            ws_tx,
            event_tx,
        })
    }

    async fn handle_key_event(&mut self, event: KeyEvent) -> Result<bool> {
        Ok(match self.mode {
            Mode::Normal => match event.code {
                event::KeyCode::Char('i') | event::KeyCode::Char('a') => {
                    self.mode = Mode::Insert;
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
                protocol::ServerMessage::AuthSuccess(None) => {
                    self.event_tx.send(AppEvent::Notify(
                        String::from("Auth unsuccessful, please try again."),
                        Duration::from_secs(3),
                    ))?;
                    self.event_tx.send(AppEvent::SpawnAuth)?;
                }
                protocol::ServerMessage::AuthSuccess(Some(token)) => {
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
                protocol::ServerMessage::ServerNotification(text) => {
                    self.received_messages
                        .push(Span::styled(text, Style::new().gray().italic()).into());
                }
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

        let mut chat_block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title_top(
                (Span::raw(" j↓  k↑").bold().green() + Span::raw(" to scroll ")).right_aligned(),
            )
            .title_top((Span::raw(" q").bold().green() + Span::raw(" to quit ")).left_aligned());
        if self.token.is_none() {
            chat_block = chat_block.title_top(
                Span::raw(" Authenticate first! ")
                    .red()
                    .into_centered_line(),
            );
        }
        Paragraph::new(self.received_messages.clone())
            .block(chat_block)
            .scroll((
                0,
                self.current_input.visual_scroll(input_area.width as usize) as u16,
            ))
            .wrap(ratatui::widgets::Wrap { trim: true })
            .render(chat_area, frame.buffer_mut());

        let input_block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title_top(if self.mode == Mode::Normal {
                Span::raw(" a/i").bold().green() + Span::raw(" to enter INSERT mode ")
            } else {
                Span::raw(" <ESC>").bold().green() + Span::raw(" to exit INSERT mode ")
            })
            .title_alignment(ratatui::layout::Alignment::Right);
        Paragraph::new(self.current_input.value())
            .block(if self.mode == Mode::Insert {
                input_block.blue()
            } else {
                input_block
            })
            .wrap(ratatui::widgets::Wrap { trim: false })
            .render(input_area, frame.buffer_mut());
    }

    async fn handle_event(&mut self, event: AppEvent, is_focused: bool) -> Result<bool> {
        Ok(match event {
            AppEvent::KeyEvent(key_event) if is_focused => self.handle_key_event(key_event).await?,
            AppEvent::WsMessage(msg) => self.handle_ws_message(msg).await?,
            _ => false,
        })
    }
}
