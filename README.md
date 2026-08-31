# ampel

Drive a [Cleware USB traffic light](https://www.cleware-shop.de/) from Rust.

| Crate | What it is |
|---|---|
| [`cleware/`](cleware) | Safe wrapper around the vendor Cleware C/C++ driver — `Color`, `Light`, `set()` |
| [`co2ampel/`](co2ampel) (`co2ampel`) | Shows **CO2 ppm** from an MQTT topic on the light |

The vendor driver in [`vendor/cleware/`](vendor/cleware) is compiled straight into the `cleware` crate by [`cleware/build.rs`](cleware/build.rs) and reached through one `extern "C"` shim, so consumers link it instead of shelling out to `USBswitchCmd`. It talks to the light over the kernel's hiddev interface (`/dev/usb/hiddev*`).

Rate limiting lives in `cleware::Light`: the hardware is only switched when the colour actually changes, never faster than a cooldown, and re-asserted periodically so the light recovers after being re-plugged. Ask for a colour as often as you like.

```rust
use cleware::{Color, Light};
use std::time::Duration;

let mut light = Light::new(Duration::from_secs(3), Duration::from_secs(30));
light.request(Color::Yellow)?;   // switches
light.request(Color::Yellow)?;   // no USB traffic
cleware::set(Color::Off)?;       // one-shot, no rate limiting
```

Also using this crate: [claude-peripherals](https://github.com/aa-ko/claude-peripherals) drives the same light from Claude Code session state.

## Layout

```
cleware/             the wrapper crate (Color, Light, set)
co2ampel/            co2ampel — CO2 ppm via MQTT on the light
vendor/cleware/      vendor Cleware C/C++ driver (GPL-3.0), compiled into cleware/
packaging/           udev rule for non-root access to the light
Cross.toml           container image + env for the ARM cross build
Taskfile.yml         build/run tasks
```

## Prerequisites

- A **Rust toolchain** (`cargo`) and [`task`](https://taskfile.dev).
- A **C/C++ compiler** (`gcc` + `g++`) — `cleware/build.rs` compiles `vendor/cleware/`.
- A **Cleware USB traffic light** plugged in.
- **udev rules** for non-root device access — `task install-udev` installs `packaging/99-cleware.rules`.

### For `co2ampel`

- An **MQTT broker** and a sensor publishing CO2 ppm to a topic, as a bare number (`812`) or a JSON object (`{"co2":812}`, see `--json-field`).
- The sensor side is [co2-monitor-rs](https://github.com/aa-ko/co2-sensor-rs): its `co2monitor-cli` reads a TFA Dostmann AIRCO2NTROL MINI over HID and publishes bare ppm numbers to the topic given by `--c-topic`. Point `co2ampel --topic` at that same topic; no `--json-field` needed.
- Plain TCP only. TLS is deliberately not compiled in — add `features = ["use-rustls"]` to the `rumqttc` dependency if a broker needs it.

### For the Raspberry Pi

[`cross`](https://github.com/cross-rs/cross) (`cargo install cross`) and a working **Docker** or Podman, then `task arm`. Pi 3 and Pi 5 both run 64-bit Raspberry Pi OS, so `aarch64-unknown-linux-gnu` covers both.

`Cross.toml` pins the older `0.2.5` container image on purpose: the default `:main` image is built on Ubuntu 24.04 and produces binaries that need glibc 2.39, which Raspberry Pi OS does not have. It also passes through the variables `task arm` uses to clear host-only cargo settings, because cross mounts `~/.cargo/config.toml` into a container that has neither sccache nor clang/mold.

## Build & run

```sh
task build           # both crates (the vendor C++ comes along via build.rs)
task test            # all workspace tests
task arm             # cross-compile co2ampel for a 64-bit Pi

task co2 -- --host pi --topic buero-nord/co2sensor/co2
task install-udev    # udev rule for the light (needs sudo)
```

`co2ampel --help` lists every flag: broker, credentials, topic, ppm thresholds, staleness timeout, and the switching limits.

## Deploying to a Raspberry Pi

`task arm`, copy `target/aarch64-unknown-linux-gnu/release/co2ampel` to the Pi, then install the unit and its environment file as described at the top of [`packaging/co2ampel.service`](packaging/co2ampel.service). Broker, topic and thresholds live in `/etc/default/co2ampel` (from [`packaging/co2ampel.env`](packaging/co2ampel.env)) so no credentials end up in the unit, and the service turns the light off when it stops.

The sensor half deploys the same way from [co2-monitor-rs](https://github.com/aa-ko/co2-sensor-rs) (`just build-pi`, then its `co2-monitor.service`).

## Testing with mqttui

[`mqttui`](https://github.com/EdJoPaTo/mqttui) is the quickest way to drive the light without touching the sensor. Watch what the sensor actually publishes:

```sh
export MQTTUI_BROKER=mqtt://homeassistant.intern.atmina.systems
mqttui                                  # browse the whole broker
mqttui log 'buero-nord/co2sensor/#'   # follow both readings
```

Fake readings, so you can see every colour without breathing on the sensor:

```sh
mqttui publish buero-nord/co2sensor/co2 700    # green
mqttui publish buero-nord/co2sensor/co2 1000   # yellow
mqttui publish buero-nord/co2sensor/co2 1500   # red

mqttui publish -r buero-nord/co2sensor/status offline   # light goes off at once
mqttui publish -r buero-nord/co2sensor/status online    # ppm takes over again
```

Run `co2ampel` against the same topic while doing that:

```sh
task co2 -- --host homeassistant.intern.atmina.systems \
  --topic buero-nord/co2sensor/co2 --status-topic buero-nord/co2sensor/status
```

Each switch is logged with the ppm that caused it. Remember the 3 s cooldown between physical switches: publish faster than that and only the newest colour lands.

## Colour mapping (`co2ampel`)

| Reading | Colour |
|---|---|
| ppm < `--warn-ppm` (900) | 🟢 green — air is fine |
| ppm < `--alarm-ppm` (1200) | 🟡 yellow — open a window soon |
| above that | 🔴 red — open a window |
| no reading for `--stale-secs` (300) | ⚫ off — never show green for a dead sensor |
| sensor published `offline` on `--status-topic` | ⚫ off — immediately, no waiting |

An `--status-topic` beats any reading: the broker replays retained readings from before an outage, and a stale number must not look like current air. [co2-monitor-rs](https://github.com/aa-ko/co2-sensor-rs) publishes `online` there on connect and registers `offline` as its last will, so the broker speaks for the sensor even when it loses power.

## License

`vendor/cleware/` is the vendor's GPL-3.0 driver (see `vendor/cleware/gpl-3.0.txt`). Everything here links it, so the whole repo is **GPL-3.0-only**.
