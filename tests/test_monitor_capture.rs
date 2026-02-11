// 集成测试：按索引截取每个监视器

use hdrcapture::capture::find_monitor;
use hdrcapture::pipeline::CapturePipeline;

fn capture_monitor(index: usize) {
    let mut pipeline = CapturePipeline::monitor(index).unwrap();
    let frame = pipeline.capture().unwrap();

    assert!(frame.width > 0 && frame.height > 0);
    assert!(
        frame.data.iter().any(|&b| b != 0),
        "Monitor {} captured all black",
        index
    );
    println!("Monitor {}: {}x{}", index, frame.width, frame.height);

    frame
        .save(format!("tests/results/monitor_{}.png", index))
        .unwrap();
}

#[test]
fn test_capture_monitor_0() {
    capture_monitor(0);
}

#[test]
fn test_capture_monitor_1() {
    if find_monitor(1).is_err() {
        println!("SKIPPED: only one monitor detected");
        return;
    }
    capture_monitor(1);
}

#[test]
fn test_capture_monitor_2() {
    if find_monitor(2).is_err() {
        println!("SKIPPED: only two monitors detected");
        return;
    }
    capture_monitor(2);
}
