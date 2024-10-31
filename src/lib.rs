// https://github.com/FabianLars/tauri-plugin-oauth/blob/v2/src/lib.rs
use std::{
    borrow::Cow,
    io::{prelude::*, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    thread,
};

use tauri::{
    plugin::{Builder, TauriPlugin},
    Runtime,
};

const EXIT: [u8; 4] = [1, 3, 3, 7];

/// Starts the localhost (using 127.0.0.1) server. Returns the port its listening on.
///
/// Because of the unprotected localhost port, you _must_ verify the URL in the handler function.
///
/// # Arguments
///
/// * `handler` - Closure which will be executed on a successful connection. It receives the full URL as a String.
///
/// # Errors
///
/// - Returns `std::io::Error` if the server creation fails.
///
/// # Panics
///
/// The seperate server thread can panic if its unable to send the html response to the client. This may change after more real world testing.
// pub fn start<F: FnMut(String) + Send + 'static>(handler: F) -> Result<u16, std::io::Error> {
//     start_with_config(OauthConfig::default(), handler)
// }

/// The optional server config.
#[derive(Default, serde::Deserialize)]
pub struct OauthConfig {
    /// An array of hard-coded ports the server should try to bind to.
    /// This should only be used if your oauth provider does not accept wildcard localhost addresses.
    ///
    /// Default: Asks the system for a free port.
    pub ports: Option<Vec<u16>>,
    /// Optional static html string send to the user after being redirected.
    /// Keep it self-contained and as small as possible.
    ///
    /// Default: `"<html><body>Please return to the app.</body></html>"`.
    pub response: Option<Cow<'static, str>>,
}

/// Starts the localhost (using 127.0.0.1) server. Returns the port its listening on.
///
/// Because of the unprotected localhost port, you _must_ verify the URL in the handler function.
///
/// # Arguments
///
/// * `config` - Configuration the server should use, see [`OauthConfig.]
/// * `handler` - Closure which will be executed on a successful connection. It receives the full URL as a String.
///
/// # Errors
///
/// - Returns `std::io::Error` if the server creation fails.
///
/// # Panics
///
/// The seperate server thread can panic if its unable to send the html response to the client. This may change after more real world testing.
pub fn start_with_config<F: FnMut(String) + Send + 'static>(
    config: OauthConfig,
    mut handler: F,
) -> Result<u16, std::io::Error> {
    let listener = match config.ports {
        Some(ports) => TcpListener::bind(
            ports
                .iter()
                .map(|p| SocketAddr::from(([127, 0, 0, 1], *p)))
                .collect::<Vec<SocketAddr>>()
                .as_slice(),
        ),
        None => TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))),
    }?;

    let port = listener.local_addr()?.port();

    thread::spawn(move || {
        for conn in listener.incoming() {
            match conn {
                Ok(conn) => {
                    if let Some(content) = handle_connection(conn) {
                        // Using an empty string to communicate that a shutdown was requested.
                        if !content.is_empty() {
                            handler(content);
                        } else {
                            break;
                        }
                    }
                }
                Err(err) => {
                    log::error!("Error reading incoming connection: {}", err.to_string());
                }
            }
        }
    });

    Ok(port)
}

fn handle_connection(mut conn: TcpStream) -> Option<String> {
    let mut buffer = [0; 4048];
    let read_result = conn.read(&mut buffer);
    if let Err(io_err) = &read_result {
        log::error!("Error reading incoming connection: {}", io_err.to_string());
    };

    let read_byte = read_result.unwrap();

    let mut headers = [httparse::EMPTY_HEADER; 100];
    let mut request = httparse::Request::new(&mut headers);
    request.parse(&buffer).ok();

    let path = request
        .path
        .map(|v| v.to_string().to_owned())
        .unwrap_or("".to_string());

    if path == "/exit" {
        return Some(String::new());
    };

    let mut content_length = 0;
    for header in &headers {
        if header.name == "Content-Length" {
            content_length = String::from_utf8_lossy(header.value)
                .to_string()
                .parse()
                .unwrap();
        }
    }

    let mut request_body = None;
    if content_length > 0 && path == "/submit" {
        let request_string = String::from_utf8_lossy(&buffer[..read_byte]);
        let parts: Vec<&str> = request_string.splitn(2, "\r\n\r\n").collect();
        let mut content = parts.get(1).unwrap_or(&"").to_string();
        let not_read_bytes = content_length - content.len();
        if not_read_bytes > 0 {
            let mut body_buffer = vec![0; not_read_bytes];
            conn.read_exact(&mut body_buffer).unwrap();
            let missing = String::from_utf8_lossy(&body_buffer).to_string();
            content.push_str(&missing);
        }

        if content.is_empty() == false {
            request_body = Some(content);
        }
    }

    let response = "true".to_string();

    conn.write_all(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Allow-Methods: POST, GET, OPTIONS\r\nAccess-Control-Allow-Credentials: true\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json; charset=utf-8\r\ncache-control: max-age=0, private, must-revalidate\r\n\r\n{}",
            response.len(),
            response
        )
        .as_bytes(),
    )
    .unwrap();
    conn.flush().unwrap();

    request_body
}

/// Stops the currently running server behind the provided port without executing the handler.
/// Alternatively you can send a request to http://127.0.0.1:port/exit
///
/// # Errors
///
/// - Returns `std::io::Error` if the server couldn't be reached.
pub fn cancel(port: u16) -> Result<(), std::io::Error> {
    // Using tcp instead of something global-ish like an AtomicBool,
    // so we don't have to dive into the set_nonblocking madness.
    let mut stream = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port)))?;
    stream.write_all(&EXIT)?;
    stream.flush()?;

    Ok(())
}

mod plugin_impl {
    use tauri::{Emitter, Manager, Runtime, Window};

    #[tauri::command]
    pub fn start<R: Runtime>(
        window: Window<R>,
        config: Option<super::OauthConfig>,
    ) -> Result<u16, String> {
        let mut config = config.unwrap_or_default();
        if config.response.is_none() {
            config.response = window
                .config()
                .plugins
                .0
                .get("oauth")
                .map(|v| v.as_str().unwrap().to_string().into());
        }

        super::start_with_config(config, move |content| {
            if let Err(emit_err) = window.emit("oauth://response", content) {
                log::error!("Error emitting oauth://response event: {}", emit_err)
            };
        })
        .map_err(|err| err.to_string())
    }

    #[tauri::command]
    pub fn cancel(port: u16) -> Result<(), String> {
        super::cancel(port).map_err(|err| err.to_string())
    }
}

/// Initializes the tauri plugin.
/// Only use this if you need the JavaScript APIs.
///
/// Note for the `start()` command: If `response` is not provided it will fall back to the config
/// in tauri.conf.json if set and will fall back to the library's default, see [`OauthConfig`].
#[must_use]
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("oauth")
        .invoke_handler(tauri::generate_handler![
            plugin_impl::start,
            plugin_impl::cancel
        ])
        .build()
}
