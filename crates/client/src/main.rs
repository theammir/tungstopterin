use std::{collections::VecDeque, time::Duration};

use color_eyre::eyre::Result;
use common::protocol;
use component::Component;
use ratatui::{
    DefaultTerminal,
    crossterm::{
        self,
        event::{self},
    },
    prelude::*,
};
use tokio::{
    net::TcpStream,
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};
use tokio_util::sync::CancellationToken;
use websocket::{
    Server, WsRecv, WsRecvHalf, WsSend, WsSendHalf, WsStream, handshake::IntoWebsocket,
    message::Message,
};

fn into_ratatui_color(color: protocol::Color) -> ratatui::style::Color {
    match color {
        protocol::Color::Text => Color::Reset,
        protocol::Color::Truecolor(r, g, b) => Color::Rgb(r, g, b),
        protocol::Color::Red => Color::Red,
        protocol::Color::Yellow => Color::Yellow,
        protocol::Color::Green => Color::Green,
        protocol::Color::Cyan => Color::Cyan,
        protocol::Color::Blue => Color::Blue,
        protocol::Color::Magenta => Color::Magenta,
        _ => Color::Reset,
    }
}

fn into_protocol_color(color: Color) -> protocol::Color {
    match color {
        Color::Reset => protocol::Color::Text,
        Color::Red => protocol::Color::Red,
        Color::Green => protocol::Color::Green,
        Color::Yellow => protocol::Color::Yellow,
        Color::Blue => protocol::Color::Blue,
        Color::Magenta => protocol::Color::Magenta,
        Color::Cyan => protocol::Color::Cyan,
        _ => protocol::Color::Text,
    }
}

pub mod component;
pub mod components;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    /// Incoming server WebSocket message.
    WsMessage(Message),
    /// Incoming terminal KeyEvent.
    KeyEvent(crossterm::event::KeyEvent),

    /// Pop from the stack, *destroying a component*, and move focus one position down.
    CompUnfocus,
    /// Move component focus one position up the stack, if possible.
    CompFocus,

    /// Spawn [components::Auth] pop-up.
    SpawnAuth,
}

#[derive(Debug, Default)]
struct ComponentStack {
    inner: VecDeque<Box<dyn Component + Send>>,
    focus: usize,
}

#[derive(Debug)]
struct App {
    should_quit: bool,

    components: ComponentStack,

    event_rx: UnboundedReceiver<AppEvent>,
    event_tx: UnboundedSender<AppEvent>,
    // TODO: Bounded sender here?
    ws_tx: UnboundedSender<Message>,

    cancel_token: CancellationToken,
}

impl App {
    fn new(ws_rx: WsRecvHalf<Server>, ws_tx: WsSendHalf<Server>) -> Self {
        let app_cancel = CancellationToken::new();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
        let ws_tx = App::spawn_ws_sender(ws_tx);

        let app = App {
            should_quit: false,
            components: ComponentStack::default(),
            event_tx,
            event_rx,
            ws_tx,
            cancel_token: app_cancel,
        };
        app.spawn_event_emitter(ws_rx, app.cancel_token.child_token());
        app
    }

    fn spawn_event_emitter(&self, mut ws_rx: WsRecvHalf<Server>, event_cancel: CancellationToken) {
        let inner_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let event_tx = inner_tx;
            loop {
                if event_cancel.is_cancelled() {
                    break;
                }
                if matches!(crossterm::event::poll(Duration::from_millis(50)), Ok(true)) {
                    if let Ok(crossterm::event::Event::Key(event)) = crossterm::event::read() {
                        _ = event_tx.send(AppEvent::KeyEvent(event));
                    }
                }
            }
        });

        let inner_tx = self.event_tx.clone();
        tokio::spawn(async move {
            while let Ok(msg) = ws_rx.receive().await {
                _ = inner_tx.send(AppEvent::WsMessage(msg));
            }
        });
    }

    fn spawn_ws_sender(mut ws_tx: WsSendHalf<Server>) -> UnboundedSender<Message> {
        let (shared_ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        tokio::spawn(async move {
            loop {
                if let Some(msg) = ws_rx.recv().await {
                    _ = ws_tx.send(msg).await;
                }
            }
        });
        shared_ws_tx
    }

    async fn init_components(&mut self) -> Result<()> {
        self.components.inner.push_back(components::Chat::new(
            self.ws_tx.clone(),
            self.event_tx.clone(),
        ));

        for component in self.components.inner.iter_mut() {
            component.init().await?;
        }
        Ok(())
    }

    async fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.init_components().await?;
        while !self.should_quit {
            self.delegate_event().await?;
            terminal.draw(|frame| self.draw(frame))?;
        }
        Ok(())
    }

    /// Components are drawn *from the **bottom** of the stack*, as one would
    /// imagine rendering windows first and their pop-ups second.
    /// Any component is free to choose to be rendered when not in focus.
    fn draw(&mut self, frame: &mut Frame) {
        for (i, component) in self.components.inner.iter_mut().enumerate() {
            component.render(frame, frame.area(), self.components.focus == i);
        }
    }

    /// Events are handled by components *from the **top** of the stack*.
    /// If no component handles an event, the app tries to.
    async fn delegate_event(&mut self) -> Result<()> {
        if let Ok(event) = self.event_rx.try_recv() {
            let mut is_handled = false;
            for (i, component) in self.components.inner.iter_mut().enumerate().rev() {
                if let Ok(true) = component
                    .handle_event(event.clone(), self.components.focus == i)
                    .await
                {
                    is_handled = true;
                    break;
                }
            }
            if !is_handled {
                self.handle_event(event).await;
            }
        }
        Ok(())
    }

    async fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::KeyEvent(key_event) =>
            {
                #[allow(clippy::single_match)]
                match key_event.code {
                    event::KeyCode::Char('q') => {
                        self.should_quit = true;
                    }
                    _ => {}
                }
            }
            AppEvent::CompFocus => {
                self.components.focus =
                    (self.components.focus + 1).min(self.components.inner.len() - 1);
            }
            AppEvent::CompUnfocus => {
                self.components.inner.remove(self.components.focus);
                self.components.focus = self.components.focus.saturating_sub(1);
            }
            AppEvent::SpawnAuth => {
                let mut auth = components::Auth::new(self.ws_tx.clone(), self.event_tx.clone());
                if auth.init().await.is_ok() {
                    self.components.inner.push_back(auth);
                    _ = self.event_tx.send(AppEvent::CompFocus);
                }
            }
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let mut ws = WsStream::from_stream(TcpStream::connect("127.0.0.1:1337").await?);
    ws.try_upgrade().await?;
    let (ws_rx, ws_tx) = ws.into_split();

    let mut terminal = ratatui::init();
    let mut app = App::new(ws_rx, ws_tx);
    app.run(&mut terminal).await?;

    ratatui::restore();
    app.cancel_token.cancel();
    Ok(())
}
