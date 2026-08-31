//! co2ampel — show CO2 ppm from an MQTT topic on a Cleware USB traffic light.
//!
//! Subscribes to one sensor topic and maps the reading onto the light:
//!   ppm <  --warn-ppm   -> GREEN   (air is fine)
//!   ppm <  --alarm-ppm  -> YELLOW  (open a window soon)
//!   otherwise           -> RED     (open a window)
//!   no reading for --stale-secs -> OFF (never show green for a dead sensor)
//!
//! With --status-topic, the sensor's own availability wins over any reading: a
//! sensor that reports "offline" (or whose last will the broker publishes) turns
//! the light off at once, instead of waiting out --stale-secs.
//!
//! Rate limiting lives in `cleware::Light`, so a chatty sensor cannot hammer the
//! USB device.

use std::time::{Duration, Instant};

use clap::Parser;
use cleware::{Color, Light};
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};

/// Keep-alive doubles as our clock: the broker's ping traffic wakes the event
/// loop regularly, which is when the staleness check runs.
const KEEP_ALIVE: Duration = Duration::from_secs(15);
const RECONNECT_WAIT: Duration = Duration::from_secs(5);

/// Payloads on the sensor's availability topic.
const PAYLOAD_ONLINE: &[u8] = b"online";
const PAYLOAD_OFFLINE: &[u8] = b"offline";

#[derive(Parser)]
#[command(
    about = "Show CO2 ppm from an MQTT sensor topic on a Cleware USB traffic light",
    version
)]
struct Args {
    /// MQTT broker host.
    #[arg(long, default_value = "localhost", env = "CO2AMPEL_HOST")]
    host: String,
    /// MQTT broker port.
    #[arg(long, default_value_t = 1883, env = "CO2AMPEL_PORT")]
    port: u16,
    /// Topic carrying the CO2 reading.
    #[arg(long, env = "CO2AMPEL_TOPIC", required_unless_present = "off")]
    topic: Option<String>,
    /// Broker username (password required alongside it).
    #[arg(long, env = "CO2AMPEL_USERNAME")]
    username: Option<String>,
    /// Broker password.
    #[arg(long, env = "CO2AMPEL_PASSWORD")]
    password: Option<String>,
    /// MQTT client id.
    #[arg(long, default_value = "co2ampel")]
    client_id: String,
    /// Availability topic of the sensor ("online" / "offline"). Without it, only
    /// --stale-secs detects a dead sensor.
    #[arg(long, env = "CO2AMPEL_STATUS_TOPIC")]
    status_topic: Option<String>,
    /// Field to read when the payload is a JSON object.
    #[arg(long, default_value = "co2")]
    json_field: String,
    /// Below this ppm the light is green.
    #[arg(long, default_value_t = 900.0, env = "CO2AMPEL_WARN_PPM")]
    warn_ppm: f64,
    /// At or above this ppm the light is red.
    #[arg(long, default_value_t = 1200.0, env = "CO2AMPEL_ALARM_PPM")]
    alarm_ppm: f64,
    /// Turn the light off after this long without a reading.
    #[arg(long, default_value_t = 300, env = "CO2AMPEL_STALE_SECS")]
    stale_secs: u64,
    /// Minimum seconds between physical switches (the light is fragile).
    #[arg(long, default_value_t = 3)]
    cooldown_secs: u64,
    /// Re-assert the current color this often so the light survives a re-plug.
    #[arg(long, default_value_t = 30)]
    reassert_secs: u64,
    /// Turn the light off and exit, connecting to no broker.
    #[arg(long)]
    off: bool,
}

/// ppm thresholds, resolved from the CLI once so the mapping stays a pure
/// function we can test.
struct Thresholds {
    warn: f64,
    alarm: f64,
}

fn decide(ppm: f64, t: &Thresholds) -> Color {
    if ppm < t.warn {
        Color::Green
    } else if ppm < t.alarm {
        Color::Yellow
    } else {
        Color::Red
    }
}

/// What the sensor last said about itself. Assumed `Online` until it says
/// otherwise, so a broker without an availability topic behaves as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Availability {
    Online,
    Offline,
}

fn parse_availability(payload: &[u8]) -> Option<Availability> {
    match payload {
        PAYLOAD_ONLINE => Some(Availability::Online),
        PAYLOAD_OFFLINE => Some(Availability::Offline),
        _ => None,
    }
}

/// The colour to show. An offline sensor beats any reading it left behind — the
/// broker replays retained readings from before the outage, and a stale number
/// must not look like current air.
fn show(availability: Availability, reading: Option<Color>) -> Color {
    match availability {
        Availability::Offline => Color::Off,
        Availability::Online => reading.unwrap_or(Color::Off),
    }
}

/// Read a ppm value from a raw MQTT payload: either a bare number
/// (`"812"`, `"812.4"`) or a JSON object carrying it under `field`.
fn parse_ppm(payload: &[u8], field: &str) -> Result<f64, String> {
    let text = std::str::from_utf8(payload).map_err(|_| "payload is not UTF-8".to_string())?;
    let text = text.trim();
    if let Ok(ppm) = text.parse::<f64>() {
        return Ok(ppm);
    }
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("payload is neither number nor JSON: {e}"))?;
    json.get(field)
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| format!("JSON payload has no numeric `{field}`"))
}

fn main() {
    let args = Args::parse();
    if args.off {
        if let Err(e) = cleware::set(Color::Off) {
            eprintln!("[co2ampel] {e}");
            std::process::exit(1);
        }
        return;
    }
    // Guaranteed by `required_unless_present` on the flag.
    let topic = args.topic.expect("--topic is required without --off");
    let thresholds = Thresholds {
        warn: args.warn_ppm,
        alarm: args.alarm_ppm,
    };
    let stale_after = Duration::from_secs(args.stale_secs);

    let mut options = MqttOptions::new(&args.client_id, &args.host, args.port);
    options.set_keep_alive(KEEP_ALIVE);
    if let (Some(user), Some(password)) = (&args.username, &args.password) {
        options.set_credentials(user, password);
    }

    let (client, mut connection) = Client::new(options, 10);
    let mut light = Light::new(
        Duration::from_secs(args.cooldown_secs),
        Duration::from_secs(args.reassert_secs),
    );

    eprintln!(
        "[co2ampel] {}:{} topic `{}`; green <{} <=yellow< {} <=red",
        args.host, args.port, topic, args.warn_ppm, args.alarm_ppm
    );

    let mut last_reading: Option<Instant> = None;
    let mut reading: Option<Color> = None;
    let mut availability = Availability::Online;
    for event in connection.iter() {
        match event {
            // Subscribe on every (re)connect, so a dropped session resubscribes.
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                for topic in [Some(&topic), args.status_topic.as_ref()].into_iter().flatten() {
                    if let Err(e) = client.subscribe(topic, QoS::AtMostOnce) {
                        eprintln!("[co2ampel] subscribe to `{topic}` failed: {e}");
                    }
                }
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                if Some(&publish.topic) == args.status_topic.as_ref() {
                    match parse_availability(&publish.payload) {
                        Some(next) => {
                            if next != availability {
                                eprintln!("[co2ampel] sensor is {next:?}");
                            }
                            availability = next;
                        }
                        None => eprintln!(
                            "[co2ampel] {}: unknown availability payload",
                            publish.topic
                        ),
                    }
                } else {
                    match parse_ppm(&publish.payload, &args.json_field) {
                        Ok(ppm) => {
                            let color = decide(ppm, &thresholds);
                            if Some(color) != reading {
                                eprintln!("[co2ampel] {ppm:.0} ppm -> {color}");
                            }
                            reading = Some(color);
                            last_reading = Some(Instant::now());
                        }
                        Err(e) => eprintln!("[co2ampel] {}: {e}", publish.topic),
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[co2ampel] connection lost ({e}), retrying");
                std::thread::sleep(RECONNECT_WAIT);
            }
        }

        // Backstop for a sensor that stops publishing without saying goodbye.
        if last_reading.map_or(false, |t| t.elapsed() >= stale_after) {
            eprintln!("[co2ampel] no reading for {}s -> off", stale_after.as_secs());
            reading = None;
            last_reading = None;
        }

        if let Err(e) = light.request(show(availability, reading)) {
            eprintln!("[co2ampel] {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Thresholds = Thresholds {
        warn: 900.0,
        alarm: 1200.0,
    };

    #[test]
    fn thresholds_map_to_colors() {
        assert_eq!(decide(420.0, &T), Color::Green);
        assert_eq!(decide(899.9, &T), Color::Green);
        assert_eq!(decide(900.0, &T), Color::Yellow);
        assert_eq!(decide(1199.9, &T), Color::Yellow);
        assert_eq!(decide(1200.0, &T), Color::Red);
        assert_eq!(decide(2500.0, &T), Color::Red);
    }

    #[test]
    fn parses_bare_number_payload() {
        assert_eq!(parse_ppm(b"812", "co2").unwrap(), 812.0);
        assert_eq!(parse_ppm(b" 812.5\n", "co2").unwrap(), 812.5);
    }

    #[test]
    fn parses_json_field_payload() {
        let payload = br#"{"temperature":21.5,"co2":734}"#;
        assert_eq!(parse_ppm(payload, "co2").unwrap(), 734.0);
        assert!(parse_ppm(payload, "ppm").is_err());
    }

    #[test]
    fn rejects_garbage_payload() {
        assert!(parse_ppm(b"n/a", "co2").is_err());
    }

    #[test]
    fn offline_sensor_beats_its_last_reading() {
        // The broker replays a retained ppm from before the outage; it must not
        // outrank the retained "offline" that came with it.
        assert_eq!(show(Availability::Offline, Some(Color::Green)), Color::Off);
        assert_eq!(show(Availability::Online, Some(Color::Green)), Color::Green);
    }

    #[test]
    fn no_reading_yet_is_off() {
        assert_eq!(show(Availability::Online, None), Color::Off);
    }

    #[test]
    fn parses_availability_payloads() {
        assert_eq!(parse_availability(b"online"), Some(Availability::Online));
        assert_eq!(parse_availability(b"offline"), Some(Availability::Offline));
        assert_eq!(parse_availability(b"ONLINE"), None);
    }
}
