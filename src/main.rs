//! OpenMouse server: your phone as a wireless mouse, keyboard and trackpad.
//!
//! Run it, scan the QR code with your phone (same WiFi), and the remote
//! opens in the phone's browser. No accounts, no cloud, no app store.

mod input;
mod server;
mod tray;
#[cfg(windows)]
mod update;

use std::io::Cursor;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use image::{ImageFormat, Luma};
use qrcode::{Color, QrCode};

use server::AppState;

#[derive(Parser, Debug)]
#[command(name = "openmouse", about = "Phone as wireless mouse/keyboard/trackpad")]
struct Args {
    /// Port to listen on
    #[arg(long, default_value_t = 7890)]
    port: u16,
    /// Interface to bind
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
    /// Don't print the QR code in the terminal
    #[arg(long)]
    no_qr: bool,
    /// Show the console window with the QR code and logs (hidden by default;
    /// the tray icon is the UI now)
    #[arg(long)]
    console: bool,
    /// Deprecated: tray mode is now the default. Kept (hidden) so existing
    /// autostart entries that pass --tray keep working.
    #[arg(long, hide = true)]
    tray: bool,
    /// Verbose logging
    #[arg(short, long)]
    verbose: bool,
}

fn generate_token() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    (0..32)
        .map(|_| ALPHABET[rand::random::<usize>() % ALPHABET.len()] as char)
        .collect()
}

fn lan_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            Ok(s.local_addr()?.ip().to_string())
        })
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

fn qr_png_bytes(url: &str) -> Vec<u8> {
    let code = QrCode::new(url).expect("QR encode");
    let img = code
        .render::<Luma<u8>>()
        .module_dimensions(8, 8)
        .build();
    let mut buf = Vec::new();
    image::DynamicImage::ImageLuma8(img)
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .expect("PNG encode");
    buf
}

/// Print the QR code compactly: two QR modules per terminal row using
/// half-block characters, with a 1-module quiet zone. Much smaller than
/// the classic two-chars-per-module rendering — fits small terminals/TVs.
fn print_qr_ascii(url: &str) {
    let code = QrCode::new(url).expect("QR encode");
    let w = code.width() as i32;
    println!();
    for y in (-1..=w).step_by(2) {
        for x in -1..=w {
            let top = is_dark(&code, x, y);
            let bottom = is_dark(&code, x, y + 1);
            print!(
                "{}",
                match (top, bottom) {
                    (true, true) => "█",
                    (true, false) => "▀",
                    (false, true) => "▄",
                    (false, false) => " ",
                }
            );
        }
        println!();
    }
    println!();
}

fn is_dark(code: &QrCode, x: i32, y: i32) -> bool {
    let w = code.width() as i32;
    if x < 0 || y < 0 || x >= w || y >= w {
        return false; // quiet zone
    }
    code[(x as usize, y as usize)] == Color::Dark
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // The console is hidden by default — the tray icon is the UI now, and
    // pairing happens via its "Show QR code" menu item. --console keeps the
    // window visible. Hide first, before any output, to avoid a flash.
    if !args.console {
        tray::hide_console();
    }

    if args.verbose {
        eprintln!("[openmouse] verbose logging on");
    }

    let token = generate_token();
    let ip = lan_ip();
    let pair_url = pair_url_for(&ip, args.port, &token);
    let qr_png = qr_png_bytes(&pair_url);

    println!("\n  OpenMouse server running");
    println!("  Backend : {}", input::backend_name());
    println!("  Phone   : {pair_url}\n");
    if !args.no_qr {
        print_qr_ascii(&pair_url);
    }
    println!("  Scan the QR with your phone (same WiFi). Ctrl+C to stop.\n");

    // System tray icon (Windows only; no-op elsewhere). Its thread is shut
    // down cleanly below so no ghost icon is left behind. With no console,
    // the QR popup is shown on launch as the pairing UI.
    let (tray, mut exit_rx) = tray::spawn(pair_url.clone(), !args.console);

    let state = Arc::new(AppState {
        token,
        pair_url,
        qr_png,
        clients: AtomicUsize::new(0),
    });

    // When a phone pairs, dismiss the QR popup — its job is done. Cheap poll
    // of the client counter; the tray thread does the actual window teardown.
    let notifier = tray.notifier();
    let watch_state = state.clone();
    tokio::spawn(async move {
        let mut last = 0usize;
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let n = watch_state.clients.load(Ordering::Relaxed);
            if n > last {
                last = n;
                notifier.close_qr_popup();
            }
        }
    });

    tokio::select! {
        r = server::run(&args.host, args.port, state) => {
            tray.shutdown();
            if let Err(e) = r {
                eprintln!("[openmouse] server error: {e}");
                std::process::exit(1);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\n  Shutting down.");
            tray.shutdown();
        }
        _ = &mut exit_rx => {
            println!("\n  Shutting down (tray exit).");
            tray.shutdown();
        }
    }

    // A successful in-app update replaced the binary on disk: relaunch it so
    // the new version takes over. Fresh pairing token, so the QR popup shows
    // as usual for re-pairing the phone.
    if tray::take_update_restart() {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).spawn();
        }
    }
}

fn pair_url_for(ip: &str, port: u16, token: &str) -> String {
    format!("http://{ip}:{port}/remote?token={token}")
}
