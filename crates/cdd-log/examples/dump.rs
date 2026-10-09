//! `cargo run -p cdd-log --example dump -- LOG`: one line per frame, `time channel id data`.
fn main() {
    let path = std::env::args().nth(1).expect("a log file");
    for f in cdd_log::read_frames(&path).expect("readable log") {
        let data: Vec<String> = f.data.iter().map(|b| format!("{b:02x}")).collect();
        println!(
            "{:.6} {} {:x}{} {}",
            f.time,
            f.channel,
            f.id,
            if f.extended { "x" } else { "" },
            data.join("")
        );
    }
}
