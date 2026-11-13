//! Headless client: connect to a server and log everything for a fixed time (test harness).

use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

use anyhow::Result;
use gs_client::auth::LanAuthenticator;
use gs_client::postrun::NoWeapons;
use gs_client::voice::NoVoice;
use gs_client::{Client, ClientConfig, ClientEvent, State};
use gs_fs::FileSystem;
use gs_protocol::ServerMessage;

pub struct HeadlessOptions {
    pub address: String,
    pub name: String,
    pub seconds: f64,
    /// Local UDP port; 0 = ephemeral.
    pub port: u16,
    /// Commands forwarded to the server once in game, one per second.
    pub commands: Vec<String>,
}

pub fn run(fs: FileSystem, opts: &HeadlessOptions) -> Result<()> {
    // No frame limiter, no client.dll, no real cvar answers: a harness for local servers only.
    let target = opts
        .address
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.find_map(|a| if let std::net::SocketAddr::V4(v4) = a { Some(*v4.ip()) } else { None }));
    match target {
        Some(ip) if ip.is_loopback() || ip.is_private() || ip.is_link_local() => {}
        _ => anyhow::bail!("headless runs only against loopback or LAN servers, not {}", opts.address),
    }
    let mut config = ClientConfig::new(&opts.name);
    config.port = opts.port;
    // No client weapon code (`HUD_PostRunCmd`): the engine forces `cl_lw 0`.
    if let Err(e) = config.userinfo.set("cl_lw", "0") {
        log::warn!("userinfo cl_lw: {e}");
    }
    let mut client = Client::new(fs, config, Box::new(LanAuthenticator))?;
    let start = Instant::now();
    let mut last = 0.0;
    let mut entity_reports = 0;
    let mut commands = opts.commands.iter();
    let mut next_command_at: Option<f64> = None;
    while start.elapsed().as_secs_f64() < opts.seconds {
        let now = start.elapsed().as_secs_f64();
        client.check_client_state();
        client.send(now, now - last, gs_client::CommandFrame::own(now - last), &mut NoWeapons, &mut NoVoice);
        client.receive(&mut NoWeapons, &|name| {
            println!("[cvar query] {name}");
            "Bad CVAR request".to_owned()
        });
        last = now;
        if client.state() == State::Active {
            let due = *next_command_at.get_or_insert(now + 1.0);
            if now >= due
                && let Some(cmd) = commands.next()
            {
                println!("[send] {cmd}");
                client.send_string_cmd(cmd);
                next_command_at = Some(now + 1.0);
            }
        }
        while let Some(event) = client.poll_event() {
            match event {
                ClientEvent::StuffText(cmd) => {
                    println!("[stufftext] {}", cmd.trim_end());
                    if cmd.lines().any(|l| l.trim().eq_ignore_ascii_case("reconnect")) {
                        client.reconnect();
                    }
                }
                ClientEvent::Message(ServerMessage::UserMessage { name, data, .. }) => {
                    println!("[usermsg] {name} {} bytes", data.len());
                }
                ClientEvent::Message(ServerMessage::PacketEntities { .. }) => {}
                ClientEvent::Message(ServerMessage::Time(_)) => {}
                ClientEvent::Message(ServerMessage::ClientData) => {}
                ClientEvent::Message(msg) => println!("[svc] {msg:?}"),
                ClientEvent::Print(text) => print!("[print] {text}"),
                other => println!("[event] {other:?}"),
            }
        }
        if client.state() == State::Active && entity_reports < 3 && (now as u64).is_multiple_of(5) {
            let frame = client.protocol().current_frame();
            if frame.valid {
                entity_reports += 1;
                let cd = &frame.clientdata;
                println!(
                    "[snapshot] {} entities, origin {:?}, health {}, maxspeed {}",
                    frame.entities.len(),
                    cd.origin(),
                    cd.health(),
                    cd.maxspeed()
                );
            }
        }
        if client.state() == State::Disconnected && now > 1.0 {
            break;
        }
        // The end of the host frame (CL_AdjustClock).
        client.end_frame(now as f32, false);
        std::thread::sleep(Duration::from_millis(5));
    }
    client.disconnect("headless run finished");
    println!("final state {:?}", client.state());
    Ok(())
}
