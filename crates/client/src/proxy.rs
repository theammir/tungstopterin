use common::protocol;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        TcpListener, TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
};
use websocket::{frame::Frame, message::Message};

const PROXY_ADDR: &str = "127.0.0.1:1228";

const BAD_WORDS: &[&str] = &["job", "offer", "cv", "hr", "work", "milk", "cow", "dairy"];
const DELIMITERS: &[char] = &[' ', '_', '-', '\n'];

pub async fn proxied(server: TcpStream) -> std::io::Result<TcpStream> {
    let proxy_server = TcpListener::bind(PROXY_ADDR).await?;
    let new_client = TcpStream::connect(PROXY_ADDR).await?;

    let (proxy_client, _addr) = proxy_server.accept().await?;
    tokio::spawn(async move {
        let (client_rx, client_tx) = proxy_client.into_split();
        let (server_rx, server_tx) = server.into_split();
        tokio::select! {
            _ = listen_client(client_rx, server_tx) => {}
            _ = listen_server(server_rx, client_tx) => {}
        }
    });

    Ok(new_client)
}

async fn listen_client(
    mut client_rx: OwnedReadHalf,
    mut server_tx: OwnedWriteHalf,
) -> std::io::Result<()> {
    let mut buf = vec![0u8; 8192];
    loop {
        let n = client_rx.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        server_tx.write_all(&buf[..n]).await?;
    }
    Ok(())
}

async fn listen_server(
    mut server_rx: OwnedReadHalf,
    mut client_tx: OwnedWriteHalf,
) -> std::io::Result<()> {
    let mut buf = vec![0u8; 8192];
    loop {
        let n = server_rx.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let bytes = handle_server_bytes(&buf[..n]).await;
        client_tx.write_all(&bytes).await?;
    }
    Ok(())
}

async fn handle_server_bytes(bytes: &[u8]) -> Vec<u8> {
    let bytes_vec = bytes.to_vec();
    let server_msg = Frame::try_from(bytes_vec.clone())
        .ok()
        .and_then(|frame| Message::try_from(frame).ok())
        .and_then(|msg| protocol::ServerMessage::try_from(&msg).ok());
    if server_msg.is_none() {
        return bytes_vec;
    }
    match server_msg.unwrap() {
        protocol::ServerMessage::PropagateMessage(
            protocol::MessageSender { name, color },
            text,
        ) => {
            let message: Message = protocol::ServerMessage::PropagateMessage(
                protocol::MessageSender {
                    name: censor_string(&name),
                    color,
                },
                censor_string(&text),
            )
            .into();
            let frame: Frame = message.into();
            frame.into()
        }
        _ => bytes_vec,
    }
}

fn censor_string(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut boundary = 0;
    for (i, c) in input.char_indices() {
        if DELIMITERS.contains(&c) {
            if boundary < i {
                let word = &input[boundary..i];
                if BAD_WORDS.iter().any(|w| word.eq_ignore_ascii_case(w)) {
                    output.extend(std::iter::repeat_n('#', word.chars().count()));
                } else {
                    output.push_str(word);
                }
            }
            output.push(c);
            boundary = i + c.len_utf8();
        }
    }
    if boundary < input.len() {
        let word = &input[boundary..];
        if BAD_WORDS.iter().any(|w| word.eq_ignore_ascii_case(w)) {
            output.extend(std::iter::repeat_n('#', word.chars().count()));
        } else {
            output.push_str(word);
        }
    }
    output
}
