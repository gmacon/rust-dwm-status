use std::error::Error;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;
use std::thread;

#[macro_use]
extern crate chan;
extern crate chan_signal;

extern crate chrono;
extern crate notify_rust;
extern crate systemstat;
extern crate xcb;

use chan_signal::Signal;
use systemstat::{Platform, System};
use systemstat::data::IpAddr::V4;

fn get_mail() -> Result<i32, Box<Error>> {
    let output = Command::new("notmuch")
        .arg("count")
        .arg("tag:inbox")
        .output()?;
    let inbox_count_string = String::from_utf8(output.stdout)?;
    return Ok(inbox_count_string.trim().parse()?);
}

fn mail() -> String {
    if let Ok(inbox_count) = get_mail() {
        if inbox_count > 0 {
            return format!("📧 {}", inbox_count);
        }
    }
    return "".to_string();
}

fn get_mute() -> Result<bool, Box<Error>> {
    let output = Command::new("pamixer")
        .arg("--get-mute")
        .output()?;
    let mute_string = String::from_utf8(output.stdout)?;
    return Ok(mute_string.trim() == String::from("true"));
}

fn get_volume() -> Result<i32, Box<Error>> {
    let output = Command::new("pamixer")
        .arg("--get-volume")
        .output()?;
    let volume_string = String::from_utf8(output.stdout)?;
    return Ok(volume_string.trim().parse()?);
}

fn volume() -> String {
    if let Ok(muted) = get_mute() {
        if muted {
            return "🔇".to_string()
        }
    }

    if let Ok(volume) = get_volume() {
        let speaker = match volume {
            0 ... 33 => "🔈",
            34 ... 66 => "🔉",
            _ => "🔊",
        };
        return format!("{} {}", speaker, volume)
    }
    return "".to_string();
}

fn network(sys: &System) -> String {
    if let Ok(interfaces) = sys.networks() {
        if let Some(dock_info) = interfaces.get("dock0") {
            for net in &dock_info.addrs {
                if let V4(_) = net.addr {
                    return "⇅".to_string()
                }
            }
        }
        if let Some(wireless_info) = interfaces.get("wlp58s0") {
            for net in &wireless_info.addrs {
                if let V4(_) = net.addr {
                    return "📡".to_string()
                }
            }
        }
        "".to_string()
    } else {
        "".to_string()
    }
}

fn plugged(sys: &System) -> String {
    if let Ok(plugged) = sys.on_ac_power() {
        if plugged {
            "🔌".to_string()
        } else {
            "🔋".to_string()
        }
    } else {
        "🔌".to_string()
    }
}

fn battery(sys: &System) -> String {
    if let Ok(bat) = sys.battery_life() {
        format!("{} {:.1}%", plugged(sys), bat.remaining_capacity * 100.)
    } else {
        "".to_string()
    }
}

fn ram(sys: &System) -> String {
    if let Ok(mem) = sys.memory() {
        let used = mem.total - mem.free;
        format!("▯ {}", used)
    } else {
        "▯ _".to_string()
    }
}

fn cpu(sys: &System) -> String {
    if let Ok(load) = sys.load_average() {
        format!("⚙ {:.2}", load.one)
    } else {
        "⚙ _".to_string()
    }
}

fn date() -> String {
    chrono::Local::now().format("📆 %a, %d %h ⸱ 🕓 %R").to_string()
}

fn separated(s: String) -> String {
    if s == "" { s } else { s + " ⸱ " }
}

fn status(sys: &System) -> String {
    separated(mail()) +
        &separated(volume()) +
        &separated(network(sys)) +
        &separated(battery(sys)) +
        &separated(ram(sys)) +
        &separated(cpu(sys)) +
        &date()
}

fn run_update_status(chan: mpsc::Receiver<String>) {
    let (xconn, screen_num) = xcb::Connection::connect(None).unwrap();
    let setup = xconn.get_setup();
    let screen = setup.roots().nth(screen_num as usize).unwrap();
    let root_window = screen.root();

    for status in chan.iter() {
        xcb::xproto::change_property(&xconn,
                                     xcb::xproto::PROP_MODE_REPLACE as u8,
                                     root_window,
                                     xcb::xproto::ATOM_WM_NAME,
                                     xcb::xproto::ATOM_STRING,
                                     8,
                                     status.as_bytes());
        xconn.flush();
    }
}

fn run(_sdone: chan::Sender<()>, tx_status: mpsc::Sender<String>) {
    use notify_rust::server::NotificationServer;
    let mut server = NotificationServer::new();
    let sys = System::new();

    let (tx_notification, rx_notification) = std::sync::mpsc::channel();
    thread::spawn(move || {
                           server.start(|notification| tx_notification.send(notification.clone()).unwrap())
                       });
    let mut banner = String::new();
    loop {
        let received = rx_notification.try_recv();
        if received.is_ok() {
            let notification = received.unwrap();
            banner = format!("{} {}", notification.summary, notification.body);
            tx_status.send(banner.clone()).unwrap();
            let max_timeout = 10_000; // milliseconds (1 minute)
            let mut t = notification.timeout.into();
            if t > max_timeout || t < 0 {
                t = max_timeout;
            }
            thread::sleep(Duration::from_millis(t as u64));
        }
        let next_banner = status(&sys);
        if next_banner != banner {
            banner = next_banner;
            tx_status.send(banner.clone()).unwrap();
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn main() {
    // Signal gets a value when the OS sent a INT or TERM signal.
    let signal = chan_signal::notify(&[Signal::INT, Signal::TERM]);

    // When our work is complete, send a sentinel value on `sdone`.
    let (sdone, rdone) = chan::sync(0);

    // Channel to pass status updates
    let (tx_status, rx_status) = mpsc::channel();

    thread::spawn(move || run_update_status(rx_status));

    // Run work.
    let main_tx_status = tx_status.clone();
    thread::spawn(move || run(sdone, main_tx_status));

    // Wait for a signal or for work to be done.
    chan_select! {
        signal.recv() -> signal => {
            tx_status.send(format!("rust-dwm-status stopped with signal {:?}.", signal)).unwrap();
        },
        rdone.recv() => {
            tx_status.send("rust-dwm-status: done.".to_string()).unwrap();
        }
    }
}
