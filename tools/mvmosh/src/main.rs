//! `mvmosh IN OUT --start SECONDS [options]`: a classic motion-vector datamosh of a video file.
//!
//! Up to `--start` the output is the input. At `--start` the frame freezes, and from then on it is
//! moved only by the block motion of the following frames (of the input, or of `--motion`), as a
//! decoder does when a keyframe has been dropped. After `--end` the input shows again, as when the
//! next keyframe arrives. Frames are decoded and encoded with the `ffmpeg` and `ffprobe` command
//! line tools, which must be on `PATH`; the input's audio is copied over.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use mvmosh::{Plane, Settings, estimate, warp};

const USAGE: &str = "usage: mvmosh IN OUT --start SECONDS [options]

  --start S      time the picture freezes and the mosh begins (required)
  --end S        time the input shows again (default: the mosh runs to the end)
  --motion FILE  take the motion from another clip, from its first frame (scaled to IN's size)
  --block N      block size in pixels (default 16, as H.264 macroblocks)
  --search N     largest motion searched, in pixels (default 32)
  --subpel N     1, 2 or 4: whole, half or quarter pixel motion (default 4)
  --lambda X     cost of motion; raise it to keep flat areas still (default 0.0015)
  --gain X       scale every motion vector (default 1; above 1 exaggerates the drag)
  --crf N        x264 quality of OUT, lower is better (default 16)";

struct Args {
    input: String,
    output: String,
    start: f64,
    end: Option<f64>,
    motion: Option<String>,
    settings: Settings,
    gain: f32,
    crf: u32,
}

fn fail(msg: &str) -> ! {
    eprintln!("mvmosh: {msg}\n\n{USAGE}");
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut positional = Vec::new();
    let (mut start, mut end, mut motion) = (None, None, None);
    let (mut settings, mut gain, mut crf) = (Settings::default(), 1.0, 16);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().unwrap_or_else(|| fail(&format!("{name} needs a value")));
        fn num<T: std::str::FromStr>(name: &str, v: String) -> T {
            v.parse().unwrap_or_else(|_| fail(&format!("{name}: `{v}` is not a number")))
        }
        match a.as_str() {
            "--start" => start = Some(num("--start", value("--start"))),
            "--end" => end = Some(num("--end", value("--end"))),
            "--motion" => motion = Some(value("--motion")),
            "--block" => settings.block = num("--block", value("--block")),
            "--search" => settings.search = num("--search", value("--search")),
            "--subpel" => settings.subpel = num("--subpel", value("--subpel")),
            "--lambda" => settings.lambda = num("--lambda", value("--lambda")),
            "--gain" => gain = num("--gain", value("--gain")),
            "--crf" => crf = num("--crf", value("--crf")),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            s if s.starts_with("--") => fail(&format!("unknown option `{s}`")),
            _ => positional.push(a),
        }
    }
    let [input, output]: [String; 2] = positional.try_into().unwrap_or_else(|_| fail("give an input and an output file"));
    if ![1, 2, 4].contains(&settings.subpel) {
        fail("--subpel must be 1, 2 or 4");
    }
    if settings.block < 2 {
        fail("--block must be at least 2");
    }
    Args { input, output, start: start.unwrap_or_else(|| fail("--start is required")), end, motion, settings, gain, crf }
}

/// Width, height and frame rate (as ffmpeg's `num/den` text and as a number) of a video's first
/// video stream.
fn probe(path: &str) -> (usize, usize, String, f64) {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,r_frame_rate", "-of", "csv=p=0", path])
        .output()
        .unwrap_or_else(|e| fail(&format!("can't run ffprobe: {e}")));
    let text = String::from_utf8_lossy(&out.stdout);
    let f: Vec<&str> = text.trim().split(',').collect();
    if !out.status.success() || f.len() < 3 {
        fail(&format!("ffprobe can't read `{path}`: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let (w, h) = (f[0].parse().unwrap_or(0), f[1].parse().unwrap_or(0));
    let rate = f[2].to_string();
    let fps = match rate.split_once('/') {
        Some((n, d)) => n.parse::<f64>().unwrap_or(0.0) / d.parse::<f64>().unwrap_or(1.0),
        None => rate.parse().unwrap_or(0.0),
    };
    if w == 0 || h == 0 || fps.is_nan() || fps <= 0.0 {
        fail(&format!("`{path}` has no usable video stream"));
    }
    (w, h, rate, fps)
}

/// Raw RGB8 frames out of ffmpeg.
struct Decoder {
    child: Child,
    out: ChildStdout,
}

impl Decoder {
    fn open(path: &str, size: Option<(usize, usize)>) -> Decoder {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error", "-i", path]);
        if let Some((w, h)) = size {
            cmd.args(["-vf", &format!("scale={w}:{h}")]);
        }
        let mut child =
            cmd.args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]).stdout(Stdio::piped()).spawn().unwrap_or_else(|e| fail(&format!("can't run ffmpeg: {e}")));
        let out = child.stdout.take().expect("piped stdout");
        Decoder { child, out }
    }

    /// The next frame into `buf`; false at the end of the video.
    fn next(&mut self, buf: &mut [u8]) -> bool {
        self.out.read_exact(buf).is_ok()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn open_encoder(a: &Args, w: usize, h: usize, rate: &str) -> (Child, ChildStdin) {
    let mut child = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &format!("{w}x{h}"), "-r", rate, "-i", "-"])
        .args(["-i", &a.input, "-map", "0:v", "-map", "1:a?", "-c:v", "libx264", "-crf", &a.crf.to_string(), "-pix_fmt", "yuv420p"])
        .args(["-c:a", "aac", "-shortest", &a.output])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| fail(&format!("can't run ffmpeg: {e}")));
    let stdin = child.stdin.take().expect("piped stdin");
    (child, stdin)
}

fn main() {
    let a = parse_args();
    let (w, h, rate, fps) = probe(&a.input);
    let start = (a.start * fps).round().max(0.0) as usize;
    let end = a.end.map(|e| (e * fps).round() as usize);
    let mut input = Decoder::open(&a.input, None);
    let mut motion = a.motion.as_deref().map(|m| Decoder::open(m, Some((w, h))));
    let (mut encoder, mut sink) = open_encoder(&a, w, h, &rate);

    let n = w * h * 3;
    let (mut frame, mut motion_frame, mut bytes) = (vec![0u8; n], vec![0u8; n], vec![0u8; n]);
    // While moshing: the moshed picture (f32 RGB 0–255) and the luma of the last motion frame.
    let mut mosh: Option<(Vec<f32>, Plane)> = None;
    let mut i = 0;
    while input.next(&mut frame) {
        let moshing = i >= start && end.is_none_or(|e| i < e);
        if !moshing {
            mosh = None;
            sink.write_all(&frame).unwrap_or_else(|e| fail(&format!("ffmpeg stopped taking frames: {e}")));
        } else {
            // The frame that drives the motion: the input's own, or the motion clip's next one. A
            // motion clip that has ended leaves the picture where it is.
            let driver = match motion.as_mut() {
                Some(m) => m.next(&mut motion_frame).then_some(&motion_frame),
                None => Some(&frame),
            };
            match (&mut mosh, driver) {
                (None, driver) => {
                    let first = driver.map_or_else(|| Plane::luma(&frame, w, h), |d| Plane::luma(d, w, h));
                    mosh = Some((frame.iter().map(|&b| b as f32).collect(), first));
                }
                (Some((picture, last)), Some(d)) => {
                    let cur = Plane::luma(d, w, h);
                    let field = estimate(last, &cur, &a.settings);
                    *picture = warp(picture, w, h, &field, a.gain);
                    *last = cur;
                }
                (Some(_), None) => {}
            }
            let picture = &mosh.as_ref().expect("set above").0;
            for (b, &v) in bytes.iter_mut().zip(picture) {
                *b = v.round().clamp(0.0, 255.0) as u8;
            }
            sink.write_all(&bytes).unwrap_or_else(|e| fail(&format!("ffmpeg stopped taking frames: {e}")));
        }
        i += 1;
        if i % 25 == 0 {
            eprint!("\rmvmosh: {i} frames ({:.1} s)", i as f64 / fps);
        }
    }
    drop(sink);
    let ok = encoder.wait().is_ok_and(|s| s.success());
    eprintln!("\rmvmosh: {i} frames ({:.1} s) → {}", i as f64 / fps, a.output);
    if !ok {
        fail("ffmpeg failed to write the output");
    }
    if i <= start {
        eprintln!("mvmosh: warning: --start is past the end of the input, nothing was moshed");
    }
}
