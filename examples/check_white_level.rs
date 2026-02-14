// Quick diagnostic: print SDR white level for all monitors
//
// Usage: cargo run --release --example check_white_level

use hdrcapture::capture::target::{enable_dpi_awareness, find_monitor};
use hdrcapture::color::white_level::query_sdr_white_level;

fn main() {
    enable_dpi_awareness();

    for i in 0..4 {
        match find_monitor(i) {
            Ok(hmonitor) => {
                let nits = query_sdr_white_level(hmonitor);
                let scrgb_white = nits / 80.0;
                println!(
                    "Monitor {}: SDR white = {:.1} nits (scRGB white = {:.3})",
                    i, nits, scrgb_white
                );
            }
            Err(_) => break,
        }
    }

    println!("\nExpected: Windows HDR settings 'SDR content brightness' slider value.");
    println!("If this shows 80.0 nits on an HDR monitor, the query may be failing silently.");
}
