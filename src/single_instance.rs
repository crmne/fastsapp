//! One running instance at a time.
//!
//! Two copies of Fastsapp would fight over the one thing WhatsApp allows a
//! linked device: its connection. Each takes the stream from the other,
//! forever. So a second launch does not start a second app; it asks the one
//! already running to show its window and exits.
//!
//! Detection is a listening socket bound to loopback: binding is exclusive,
//! so whoever binds is the running instance, and a later launch connects to
//! say "show yourself" before exiting. It is bound to 127.0.0.1 so no
//! firewall has an opinion about it, it speaks only to itself, and the
//! operating system releases the port when the process ends, crash included,
//! so there is no stale lock file to clean up. The same on every platform.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Loopback port that marks a running instance. Registered to nothing;
/// chosen high and out of the ephemeral range.
const INSTANCE_PORT: u16 = 47_119;

/// Every request and reply starts with this, so a foreign program that
/// happens to hold the port is never mistaken for Fastsapp.
const PREFIX: &str = "fastsapp:";
const OK_REPLY: &str = "fastsapp:ok";

pub enum Outcome {
    /// This process is the only instance. Hold the guard until it exits.
    Only(Guard),
    /// Another instance is running and has been asked to show its window.
    Surfaced,
}

/// What another launch asked the running instance to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlCommand {
    /// Bring the window forward, creating it if the app lives in the tray.
    Show,
}

/// Holds whatever marks this process as the running instance. Dropping it
/// gives that up.
pub struct Guard {
    /// Filled by other launches, drained by the app every frame.
    commands: Arc<Mutex<Vec<ControlCommand>>>,
}

impl Guard {
    /// The queue another launch's requests land in. The app drains it.
    pub fn commands(&self) -> Arc<Mutex<Vec<ControlCommand>>> {
        Arc::clone(&self.commands)
    }
}

/// Sends one verb to the running instance and checks it answered as itself.
pub fn send(verb: &str) -> std::io::Result<()> {
    send_to(INSTANCE_PORT, verb)
}

fn send_to(port: u16, verb: &str) -> std::io::Result<()> {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(format!("{PREFIX}{verb}\n").as_bytes())?;
    // The listener writes one line and closes, so read to the end and keep
    // the line.
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    if reply.lines().next() == Some(OK_REPLY) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the port is held by something other than Fastsapp",
        ))
    }
}

pub fn acquire(waker: &crate::backend::Waker) -> Outcome {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, INSTANCE_PORT)) {
        Ok(listener) => listener,
        Err(_) => {
            // Someone holds the port. Ask them to show themselves, and only
            // stand down if they answer as Fastsapp.
            if send("show").is_ok() {
                return Outcome::Surfaced;
            }
            log::warn!("port {INSTANCE_PORT} is busy but not with Fastsapp; running unguarded");
            return Outcome::Only(Guard {
                commands: Default::default(),
            });
        }
    };
    let guard = Guard {
        commands: Default::default(),
    };
    let commands = Arc::clone(&guard.commands);
    let waker = waker.clone();
    let spawned = std::thread::Builder::new()
        .name("fastsapp-instance".to_owned())
        .spawn(move || serve(listener, &commands, &waker));
    if let Err(error) = spawned {
        log::warn!("cannot listen for other launches: {error}");
    }
    Outcome::Only(guard)
}

/// Answers other launches until the listener closes. One request line and
/// one reply line per connection.
fn serve(
    listener: TcpListener,
    commands: &Mutex<Vec<ControlCommand>>,
    waker: &crate::backend::Waker,
) {
    for mut stream in listener.incoming().flatten() {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let Some(line) = read_line(&mut stream) else {
            continue;
        };
        // Not our client: say nothing and hang up.
        if let Some(command) = parse(&line) {
            let _ = stream.write_all(format!("{OK_REPLY}\n").as_bytes());
            commands
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(command);
            waker.wake();
        }
    }
}

fn parse(line: &str) -> Option<ControlCommand> {
    match line.trim_end().strip_prefix(PREFIX)? {
        "show" => Some(ControlCommand::Show),
        _ => None,
    }
}

/// Reads up to the first newline. A line too long to be one of ours, or any
/// read error, disqualifies the client.
fn read_line(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = [0u8; 256];
    let mut filled = 0;
    loop {
        if filled == buffer.len() {
            return None;
        }
        match stream.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => {
                filled += read;
                if buffer[..filled].contains(&b'\n') {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    let line = buffer[..filled].split(|&byte| byte == b'\n').next()?;
    String::from_utf8(line.to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_own_show_is_understood() {
        assert_eq!(parse("fastsapp:show\n"), Some(ControlCommand::Show));
        assert_eq!(parse("fastsapp:show"), Some(ControlCommand::Show));
        assert_eq!(parse("GET / HTTP/1.1"), None);
        assert_eq!(parse("fastsapp:frobnicate"), None);
        assert_eq!(parse(""), None);
    }

    /// The whole channel over a real socket: what a second launch sends is
    /// what the app finds in its queue.
    #[test]
    fn a_second_launch_reaches_the_queue() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port");
        let port = listener.local_addr().expect("a bound address").port();
        let commands: Arc<Mutex<Vec<ControlCommand>>> = Default::default();
        let served = {
            let commands = Arc::clone(&commands);
            let waker = crate::backend::Waker::default();
            std::thread::spawn(move || serve(listener, &commands, &waker))
        };

        send_to(port, "show").expect("answered as Fastsapp");
        // An unknown verb gets no reply at all, so the client sees a closed
        // connection rather than a command it never sent being obeyed.
        assert!(send_to(port, "frobnicate").is_err());

        assert_eq!(
            *commands.lock().expect("the queue"),
            vec![ControlCommand::Show]
        );
        drop(served);
    }
}
