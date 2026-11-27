//! `m2k bench` (owned by the engine): how fast the engine's hot path is.
//!
//! Builds a config in memory, generates MIDI traffic for it and feeds it straight into the real
//! engine (a counting output backend that sends nothing, no MIDI output), driven on this thread
//! by [`BenchEngine`] so the measurement covers the engine's own processing and not the channel
//! or the thread hop. MIDI time is simulated (`--rate`), so the engine sees a busy controller,
//! not an impossible flood. Reports throughput, the cost of a message (mean and percentiles)
//! and the allocations it makes, with the monitor on and off. A diagnostic like `simulate`,
//! not a test.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use m2k_core::{BenchEngine, Config, EngineEvent, EngineOptions, NullMidiSink};
use m2k_midi::MidiMessage;
use m2k_output::{Injector, Key, MouseButton};
use serde_json::{Value, json};

const USAGE: &str = "usage: m2k bench [options]

Feeds generated MIDI through the real engine (nothing is sent to the OS) and reports how fast it
is handled, what one message costs and how much it allocates, with the monitor on and off.

options:
  --mappings <N>    mappings in the generated config (default 300)
  --messages <N>    messages per measurement (default 1000000)
  --mix <spec>      shares of the traffic, default notes=40,cc=35,enc=10,clock=15
                    (notes = a note on and its note off a few messages later, cc = knob sweeps,
                    enc = relative encoder ticks, clock = MIDI clock)
  --rate <N>        messages per second of MIDI time (default 2000: a busy controller)
  --monitor <m>     on, off or both (default both)";

// ---------------------------------------------------------------------------------------------
// Allocation counting (only while a measurement asks for it)
// ---------------------------------------------------------------------------------------------

/// The `m2k` binary's allocator: the system one, counting allocations while
/// [`count_allocations`] is on. Off (the normal case) it costs one relaxed load per allocation.
pub struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

fn note_allocation(size: usize) {
    if COUNTING.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn count_allocations(on: bool) -> (u64, u64) {
    COUNTING.store(on, Ordering::SeqCst);
    (ALLOCATIONS.swap(0, Ordering::SeqCst), ALLOCATED_BYTES.swap(0, Ordering::SeqCst))
}

// ---------------------------------------------------------------------------------------------
// Output backend: counts, sends nothing
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Counts {
    key_downs: AtomicU64,
    key_ups: AtomicU64,
    other: AtomicU64,
}

struct CountingInjector(Arc<Counts>);

impl Injector for CountingInjector {
    fn name(&self) -> &'static str {
        "bench"
    }

    fn supports(&self, _key: Key) -> bool {
        true
    }

    fn key_down(&mut self, _key: Key) -> m2k_output::Result<()> {
        self.0.key_downs.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn key_up(&mut self, _key: Key) -> m2k_output::Result<()> {
        self.0.key_ups.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn text(&mut self, _text: &str) -> m2k_output::Result<()> {
        self.0.other.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn mouse_button(&mut self, _button: MouseButton, _down: bool) -> m2k_output::Result<()> {
        self.0.other.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn mouse_move_relative(&mut self, _dx: i32, _dy: i32) -> m2k_output::Result<()> {
        self.0.other.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn mouse_move_absolute(&mut self, _x: i32, _y: i32) -> m2k_output::Result<()> {
        self.0.other.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn scroll(&mut self, _dx: i32, _dy: i32) -> m2k_output::Result<()> {
        self.0.other.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// The generated config and traffic
// ---------------------------------------------------------------------------------------------

const DEVICE: &str = "Bench Controller";

/// Sources of the generated mappings, by kind of traffic.
#[derive(Default)]
struct Sources {
    /// (channel, note)
    notes: Vec<(u8, u8)>,
    /// (channel, controller): absolute knobs and buttons.
    knobs: Vec<(u8, u8)>,
    /// (channel, controller): relative encoders.
    encoders: Vec<(u8, u8)>,
}

/// A realistic mix: notes with Hold / Trigger / Toggle / tap-hold, knobs with Change and a
/// throttle (some with a jitter guard), encoders with Step, CC buttons, a "shift" layer and
/// mappings conditioned on it, toggles with feedback lights, and a clock divider.
fn build_config(mappings: usize) -> (Config, Sources) {
    const KEYS: [&str; 16] = [
        "KeyA",
        "KeyB",
        "KeyC",
        "KeyD",
        "KeyE",
        "KeyF",
        "KeyG",
        "KeyH",
        "F13",
        "F14",
        "F15",
        "F16",
        "ArrowLeft",
        "ArrowRight",
        "ArrowUp",
        "ArrowDown",
    ];
    let mut sources = Sources::default();
    let (mut next_note, mut next_cc) = (0usize, 0usize);
    let mut note = |sources: &mut Sources| {
        let (channel, number) = (1 + (next_note / 128) % 16, next_note % 128);
        next_note += 1;
        sources.notes.push((channel as u8, number as u8));
        json!({ "channel": channel, "event": { "type": "note", "notes": number } })
    };
    let mut cc = |list: &mut Vec<(u8, u8)>, relative: bool| {
        let (channel, number) = (1 + (next_cc / 120) % 16, next_cc % 120);
        next_cc += 1;
        list.push((channel as u8, number as u8));
        let mut event = json!({ "type": "control_change", "controller": number });
        if relative {
            event["encoding"] = json!("relative_twos_complement");
        }
        json!({ "channel": channel, "event": event })
    };
    let keys = |i: usize| json!({ "type": "keys", "keys": [KEYS[i % KEYS.len()]] });
    let mut list: Vec<Value> = Vec::with_capacity(mappings + 1);
    for i in 0..mappings {
        let id = format!("m{i}");
        let mapping = match i % 20 {
            0..=4 => json!({ "id": id, "source": note(&mut sources),
                "behavior": { "type": "hold", "target": { "type": "keys", "keys": [KEYS[i % 16]] } } }),
            5..=7 => json!({ "id": id, "source": note(&mut sources),
                "behavior": { "type": "trigger", "on_press": keys(i) } }),
            8..=11 => {
                let mut mapping = json!({ "id": id, "source": cc(&mut sources.knobs, false),
                    "behavior": { "type": "change", "action": keys(i) },
                    "rate_limit": { "interval_ms": 50 } });
                if i % 2 == 0 {
                    mapping["behavior"]["min_change"] = json!(2);
                }
                mapping
            }
            12 | 13 => json!({ "id": id, "source": cc(&mut sources.encoders, true),
                "behavior": { "type": "step", "up": keys(i), "down": keys(i + 1), "max_burst": 8 } }),
            14 => json!({ "id": id, "source": cc(&mut sources.knobs, false),
                "behavior": { "type": "trigger", "on_press": keys(i) } }),
            15 => json!({ "id": id, "source": note(&mut sources),
                "behavior": { "type": "toggle", "target": { "type": "keys", "keys": [KEYS[i % 16]] } },
                "feedback": {
                    "on": { "type": "note_on", "channel": 1, "note": i % 128, "velocity": { "type": "incoming" } },
                    "off": { "type": "note_on", "channel": 1, "note": i % 128, "velocity": { "type": "fixed", "value": 0 } } } }),
            16 => json!({ "id": id, "source": note(&mut sources),
                "behavior": { "type": "hold", "target": { "type": "layer", "layer": "shift" } } }),
            17 => json!({ "id": id, "source": note(&mut sources),
                "behavior": { "type": "trigger", "on_press": keys(i) },
                "conditions": [{ "type": "layer", "layer": "shift" }] }),
            18 => json!({ "id": id, "source": note(&mut sources),
                "behavior": { "type": "tap_hold", "tap": keys(i),
                              "hold_target": { "type": "keys", "keys": ["ShiftLeft"] } } }),
            _ => json!({ "id": id, "source": { "event": { "type": "pitch_bend" } },
                "behavior": { "type": "change", "action": keys(i) }, "rate_limit": { "interval_ms": 50 } }),
        };
        list.push(mapping);
    }
    list.push(json!({ "id": "clock", "source": { "event": { "type": "clock", "divisor": 24 } },
        "behavior": { "type": "trigger", "on_press": { "type": "keys", "keys": ["F20"] } } }));
    let text = json!({ "profiles": [{ "name": "Bench", "mappings": list }] }).to_string();
    let (config, _) = m2k_core::store::parse(&text).expect("the generated config is valid");
    (config, sources)
}

/// Tiny deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[derive(Clone, Copy)]
struct Mix {
    notes: u32,
    cc: u32,
    enc: u32,
    clock: u32,
}

#[derive(Default)]
struct Traffic {
    messages: Vec<MidiMessage>,
    notes: u64,
    cc: u64,
    enc: u64,
    clock: u64,
}

fn generate(count: usize, mix: Mix, sources: &Sources) -> Traffic {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut traffic = Traffic { messages: Vec::with_capacity(count), ..Traffic::default() };
    let total = (mix.notes + mix.cc + mix.enc + mix.clock).max(1) as usize;
    // Note offs due at a message index.
    let mut releases: std::collections::VecDeque<(usize, u8, u8)> = Default::default();
    let mut knob_values = vec![0u8; sources.knobs.len()];
    let mut knob_up = vec![true; sources.knobs.len()];
    while traffic.messages.len() < count {
        let at = traffic.messages.len();
        if let Some(&(due, channel, note)) = releases.front()
            && due <= at
        {
            releases.pop_front();
            traffic.messages.push(MidiMessage::NoteOff { channel, note, velocity: 0 });
            traffic.notes += 1;
            continue;
        }
        let pick = rng.below(total) as u32;
        if pick < mix.notes && !sources.notes.is_empty() {
            let (channel, note) = sources.notes[rng.below(sources.notes.len())];
            traffic.messages.push(MidiMessage::NoteOn { channel, note, velocity: 100 });
            releases.push_back((at + 2 + rng.below(6), channel, note));
            traffic.notes += 1;
        } else if pick < mix.notes + mix.cc && !sources.knobs.is_empty() {
            // A sweep: the same knob a few steps on.
            let k = rng.below(sources.knobs.len());
            for _ in 0..8 {
                let (channel, controller) = sources.knobs[k];
                let value = &mut knob_values[k];
                if knob_up[k] && *value >= 127 || !knob_up[k] && *value == 0 {
                    knob_up[k] = !knob_up[k];
                }
                *value = if knob_up[k] { *value + 1 } else { *value - 1 };
                traffic.messages.push(MidiMessage::ControlChange {
                    channel,
                    controller,
                    value: *value,
                });
                traffic.cc += 1;
            }
        } else if pick < mix.notes + mix.cc + mix.enc && !sources.encoders.is_empty() {
            let (channel, controller) = sources.encoders[rng.below(sources.encoders.len())];
            let value = if rng.below(2) == 0 { 1 } else { 127 };
            traffic.messages.push(MidiMessage::ControlChange { channel, controller, value });
            traffic.enc += 1;
        } else {
            traffic.messages.push(MidiMessage::Clock);
            traffic.clock += 1;
        }
    }
    traffic.messages.truncate(count);
    traffic
}

// ---------------------------------------------------------------------------------------------
// Measuring
// ---------------------------------------------------------------------------------------------

struct Measured {
    per_second: f64,
    /// Nanoseconds per message, sorted.
    costs: Vec<u32>,
    allocations: u64,
    bytes: u64,
    events: u64,
    counts: Arc<Counts>,
}

fn engine(config: &Config, monitor: bool) -> (BenchEngine, Arc<AtomicU64>, Arc<Counts>) {
    let counts = Arc::new(Counts::default());
    let events = Arc::new(AtomicU64::new(0));
    let options = EngineOptions {
        injector: Box::new(CountingInjector(counts.clone())),
        track_foreground: false,
        midi_out: Box::new(NullMidiSink),
    };
    let sink = {
        let events = events.clone();
        move |event: EngineEvent| {
            // What the runtime does with monitor events while nobody watches: drop them.
            if monitor || !matches!(event, EngineEvent::Midi(_) | EngineEvent::Fired(_)) {
                events.fetch_add(1, Ordering::Relaxed);
            }
        }
    };
    let mut bench = BenchEngine::new(config.clone(), options, sink);
    bench.set_monitoring(monitor);
    (bench, events, counts)
}

fn measure(config: &Config, traffic: &Traffic, rate: u32, monitor: bool) -> Measured {
    let step = Duration::from_nanos(1_000_000_000 / u64::from(rate.max(1)));
    let run = |each: &mut dyn FnMut(&mut BenchEngine, MidiMessage, Instant)| {
        let (mut bench, events, counts) = engine(config, monitor);
        let start = Instant::now();
        for (i, message) in traffic.messages.iter().enumerate() {
            each(&mut bench, message.clone(), start + step * i as u32);
        }
        bench.finish();
        (events.load(Ordering::Relaxed), counts)
    };
    // Throughput: nothing but the engine in the loop.
    let started = Instant::now();
    let _ = run(&mut |bench, message, at| bench.midi(DEVICE.to_string(), message, at));
    let per_second = traffic.messages.len() as f64 / started.elapsed().as_secs_f64();
    // Cost of each message.
    let mut costs = Vec::with_capacity(traffic.messages.len());
    let (events, counts) = run(&mut |bench, message, at| {
        let device = DEVICE.to_string();
        let t0 = Instant::now();
        bench.midi(device, message, at);
        costs.push(t0.elapsed().as_nanos().min(u128::from(u32::MAX)) as u32);
    });
    costs.sort_unstable();
    // Allocations inside the engine (the device name is the caller's).
    let mut allocations = 0;
    let mut bytes = 0;
    let _ = run(&mut |bench, message, at| {
        let device = DEVICE.to_string();
        count_allocations(true);
        bench.midi(device, message, at);
        let (n, b) = count_allocations(false);
        allocations += n;
        bytes += b;
    });
    Measured { per_second, costs, allocations, bytes, events, counts }
}

fn percentile(sorted: &[u32], p: f64) -> u32 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn ns(value: f64) -> String {
    if value >= 1_000_000.0 {
        format!("{:.1} ms", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.2} µs", value / 1_000.0)
    } else {
        format!("{value:.0} ns")
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn run(args: &[String]) -> i32 {
    let mut mappings = 300usize;
    let mut messages = 1_000_000usize;
    let mut rate = 2000u32;
    let mut mix = Mix { notes: 40, cc: 35, enc: 10, clock: 15 };
    let mut modes = vec![true, false];
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--mappings" => mappings = value().parse().unwrap_or(mappings).max(1),
            "--messages" => messages = value().parse().unwrap_or(messages).max(1),
            "--rate" => rate = value().parse().unwrap_or(rate).max(1),
            "--mix" => {
                for part in value().split(',') {
                    let Some((name, share)) = part.split_once('=') else { continue };
                    let share = share.trim().parse().unwrap_or(0);
                    match name.trim() {
                        "notes" => mix.notes = share,
                        "cc" => mix.cc = share,
                        "enc" => mix.enc = share,
                        "clock" => mix.clock = share,
                        other => {
                            eprintln!("--mix: unknown kind {other:?} (notes, cc, enc, clock)");
                            return 2;
                        }
                    }
                }
            }
            "--monitor" => {
                modes = match value().as_str() {
                    "on" => vec![true],
                    "off" => vec![false],
                    "both" => vec![true, false],
                    other => {
                        eprintln!("--monitor: expected on, off or both, not {other:?}");
                        return 2;
                    }
                }
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("unexpected argument {other:?}\n\n{USAGE}");
                return 2;
            }
        }
    }

    let (config, sources) = build_config(mappings);
    let traffic = generate(messages, mix, &sources);
    let built = if cfg!(debug_assertions) { "debug" } else { "release" };
    println!(
        "m2k bench ({built} build): {mappings} mappings, {} messages at {} messages/s of MIDI time",
        thousands(messages as u64),
        thousands(u64::from(rate))
    );
    println!(
        "  traffic: {} note on/off, {} CC (knob sweeps), {} encoder ticks, {} clock",
        thousands(traffic.notes),
        thousands(traffic.cc),
        thousands(traffic.enc),
        thousands(traffic.clock)
    );
    println!(
        "  {:<12} {:>11} {:>9} {:>9} {:>9} {:>9} {:>9} {:>11} {:>10} {:>10}",
        "monitor",
        "msgs/s",
        "mean",
        "p50",
        "p99",
        "p99.9",
        "max",
        "allocs/msg",
        "bytes/msg",
        "events/msg"
    );
    for monitor in modes {
        let m = measure(&config, &traffic, rate, monitor);
        let n = traffic.messages.len() as f64;
        let mean = m.costs.iter().map(|&c| f64::from(c)).sum::<f64>() / n;
        println!(
            "  {:<12} {:>11} {:>9} {:>9} {:>9} {:>9} {:>9} {:>11.2} {:>10.1} {:>10.2}",
            if monitor { "on" } else { "off" },
            thousands(m.per_second as u64),
            ns(mean),
            ns(f64::from(percentile(&m.costs, 0.5))),
            ns(f64::from(percentile(&m.costs, 0.99))),
            ns(f64::from(percentile(&m.costs, 0.999))),
            ns(f64::from(m.costs.last().copied().unwrap_or(0))),
            m.allocations as f64 / n,
            m.bytes as f64 / n,
            m.events as f64 / n,
        );
        if monitor {
            println!(
                "  {:<12} output: {} key downs, {} key ups, {} other calls",
                "",
                thousands(m.counts.key_downs.load(Ordering::Relaxed)),
                thousands(m.counts.key_ups.load(Ordering::Relaxed)),
                thousands(m.counts.other.load(Ordering::Relaxed))
            );
        }
    }
    0
}
