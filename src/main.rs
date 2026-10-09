//! OpenMouse server: your phone as a wireless mouse, keyboard and trackpad.
//!
//! Run it, scan the QR code with your phone (same WiFi), and the remote
//! opens in the phone's browser. No accounts, no cloud, no app store.

mod input;
mod server;

use std::io::Cursor;
use std::net::UdpSocket;
use std::sync::atomic::AtomicUsize;
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
    /// Open the pairing page in the default browser
    #[arg(long)]
    open_browser: bool,
    /// Don't auto-open the browser
    #[arg(long)]
    no_browser: bool,
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

fn print_qr_ascii(url: &str) {
    let code = QrCode::new(url).expect("QR encode");
    let w = code.width();
    println!();
    for y in 0..w {
        for x in 0..w {
            print!(
                "{}",
                if code[(x, y)] == Color::Dark {
                    "██"
                } else {
                    "  "
                }
            );
        }
        println!();
    }
    println!();
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

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
    print_qr_ascii(&pair_url);
    println!("  Scan the QR with your phone (same WiFi). Ctrl+C to stop.\n");

    if args.open_browser && !args.no_browser {
        let url = format!("http://127.0.0.1:{}/", args.port);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1200));
            if let Err(e) = open::that(&url) {
                eprintln!("[openmouse] couldn't open browser: {e}");
            }
        });
    }

    let state = Arc::new(AppState {
        token,
        pair_url,
        qr_png,
        clients: AtomicUsize::new(0),
    });

    if let Err(e) = server::run(&args.host, args.port, state).await {
        eprintln!("[openmouse] server error: {e}");
        std::process::exit(1);
    }
}

fn pair_url_for(ip: &str, port: u16, token: &str) -> String {
    format!("http://{ip}:{port}/remote?token={token}")
}
