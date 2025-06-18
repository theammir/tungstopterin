#![warn(clippy::pedantic)]
use std::{collections::VecDeque, sync::Arc, time::Duration};

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
use rustls_native_certs::load_native_certs;
use tokio::{
    net::TcpStream,
    sync::mpsc::{UnboundedReceiver, UnboundedSender, error::SendError},
};
use tokio_rustls::{
    TlsConnector,
    rustls::{
        self,
        pki_types::{CertificateDer, ServerName, pem::PemObject},
    },
};
use tokio_util::sync::CancellationToken;
use websocket::{
    Server, WsRecv, WsRecvHalf, WsSend, WsSendHalf, WsStream, handshake::IntoWebsocket,
    message::Message,
};

use crate::components::Urgency;

type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

fn into_ratatui_color(color: protocol::Color) -> ratatui::style::Color {
    #[allow(clippy::match_same_arms)]
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
    #[allow(clippy::match_same_arms)]
    match color {
        Color::Reset => protocol::Color::Text,
        Color::White => protocol::Color::Text,
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
    /// Incoming terminal [`KeyEvent`][`crossterm::event::KeyEvent`].
    KeyEvent(crossterm::event::KeyEvent),

    /// Pop from the stack, *destroying a component*, and move focus one position down.
    ComponentUnfocus,
    /// Move component focus one position up the stack, if possible.
    ComponentFocus,

    /// Spawn [`components::Auth`] pop-up.
    SpawnAuth,

    /// Spawn a notification for a period of time.
    Notify(Text<'static>, Urgency, Duration),
}

#[derive(Debug, Clone)]
pub struct EventSender(pub UnboundedSender<AppEvent>);
impl EventSender {
    /// Attempts to send an app event.
    ///
    /// # Errors
    ///
    /// See [`UnboundedSender::send`]
    pub fn send(&self, message: AppEvent) -> Result<(), SendError<AppEvent>> {
        self.0.send(message)
    }

    /// Attempts to send a notification with rich formatting.
    ///
    /// # Errors
    ///
    /// See [`UnboundedSender::send`]
    pub fn notify<'a>(
        &mut self,
        text: impl Into<Text<'a>>,
        urgency: Urgency,
        duration: Duration,
    ) -> Result<(), SendError<AppEvent>> {
        let text: Text<'a> = text.into();
        let owned_text: Text<'static> = text
            .lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| {
                        let new_span: Span<'static> =
                            Span::styled(span.content.into_owned(), span.style);
                        new_span
                    })
                    .collect::<Line<'static>>()
            })
            .collect();
        self.0.send(AppEvent::Notify(owned_text, urgency, duration))
    }
}

#[derive(Debug, Default)]
struct ComponentStack {
    inner: VecDeque<Box<dyn Component + Send>>,
    focus: usize,
}

impl ComponentStack {
    fn push_back(&mut self, component: Box<dyn Component + Send>) {
        self.inner.push_back(component);
    }

    fn push_after_focused(&mut self, component: Box<dyn Component + Send>) {
        self.inner.insert(self.focus + 1, component);
    }

    fn pop_focused(&mut self) {
        self.inner.remove(self.focus);
    }
}

#[derive(Debug)]
struct App {
    should_quit: bool,

    components: ComponentStack,

    event_rx: UnboundedReceiver<AppEvent>,
    event_tx: EventSender,
    // TODO: Bounded sender here?
    ws_tx: UnboundedSender<Message>,

    cancel_token: CancellationToken,
}

impl App {
    fn new(ws_rx: WsRecvHalf<Server, TlsStream>, ws_tx: WsSendHalf<Server, TlsStream>) -> Self {
        let app_cancel = CancellationToken::new();
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();
        let ws_tx = App::spawn_ws_sender(ws_tx);

        let app = App {
            should_quit: false,
            components: ComponentStack::default(),
            event_tx: EventSender(event_tx),
            event_rx,
            ws_tx,
            cancel_token: app_cancel,
        };
        app.spawn_event_emitter(ws_rx, app.cancel_token.child_token());
        app
    }

    fn spawn_event_emitter(
        &self,
        mut ws_rx: WsRecvHalf<Server, TlsStream>,
        event_cancel: CancellationToken,
    ) {
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

    fn spawn_ws_sender(mut ws_tx: WsSendHalf<Server, TlsStream>) -> UnboundedSender<Message> {
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
        self.components.push_back(components::Chat::new(
            self.ws_tx.clone(),
            self.event_tx.clone(),
        ));
        self.components.push_back(components::Notification::new());

        for component in &mut self.components.inner {
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
            AppEvent::ComponentFocus => {
                self.components.focus =
                    (self.components.focus + 1).min(self.components.inner.len() - 1);
            }
            AppEvent::ComponentUnfocus => {
                self.components.pop_focused();
                self.components.focus = self.components.focus.saturating_sub(1);
            }
            AppEvent::SpawnAuth => {
                let mut auth = components::Auth::new(self.ws_tx.clone(), self.event_tx.clone());
                if auth.init().await.is_ok() {
                    self.components.push_after_focused(auth);
                    _ = self.event_tx.send(AppEvent::ComponentFocus);
                }
            }
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // TODO: clap
    color_eyre::install()?;

    let conn = TcpStream::connect("localhost:1337").await?;
    conn.set_nodelay(true)?;

    let mut root_cert_store = rustls::RootCertStore::empty();
    for cert in load_native_certs().expect("could not load platform native certs") {
        root_cert_store.add(cert)?;
    }
    root_cert_store.add(
        CertificateDer::pem_file_iter("certs/root-ca.pem")
            .unwrap()
            .flatten()
            .next()
            .unwrap(),
    )?;

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));

    let domain = ServerName::try_from("localhost")?.to_owned();
    let conn = connector.connect(domain, conn).await?;

    let mut ws = WsStream::<Server, _>::from_stream(conn);
    ws.try_upgrade("localhost:1337").await?;
    let (ws_rx, ws_tx) = ws.into_split();

    let mut terminal = ratatui::init();
    let mut app = App::new(ws_rx, ws_tx);
    app.run(&mut terminal).await?;

    // TODO: Start closing handshake

    ratatui::restore();
    app.cancel_token.cancel();
    Ok(())
}
