# Surveillance

A Rust desktop host detects movement and loud sounds from a camera and microphone. The Android app pairs with the host, shows live video, sensor status, and event history, lets you arm or disarm detection, and receives alerts through Firebase Cloud Messaging (FCM).

Detection uses frame differencing and audio levels. The host keeps a recent camera frame and a short rolling audio buffer in memory. A detection event saves a JPEG photo and up to three seconds of audio ending at the trigger when those sensors are available. The host stores the media and event history in SQLite. The default retention period is seven days, and the host keeps media for up to 1,000 events. The host starts disarmed on a fresh install and remembers its armed state across restarts.

## Build with Nix

With Nix flakes enabled, run these commands from the repository root.

```sh
nix develop -c cargo test --locked --workspace
nix develop -c cargo test --locked -p surveillance -- --ignored
nix build
nix develop .#android -c ./android/gradlew -p android testDebugUnitTest lintDebug assembleDebug
```

The Android shell includes JDK 17, SDK 35, and ADB. The Android SDK license is accepted in the flake so Nix can install it. The Gradle wrapper validates its distribution checksum. On a NixOS machine where Gradle's downloaded AAPT2 cannot run, pass `-Pandroid.aapt2FromMavenOverride="$ANDROID_HOME/build-tools/35.0.0/aapt2"` from within the Android shell.

The host shell and Nix package are available on macOS and Linux. Windows uses native Rust 1.95 or later and FFmpeg on `PATH`, with `cargo build --locked --release` from the repository root. GitHub Actions tests Windows natively. See [device setup and platform checks](docs/setup.md) for capture backends and verification.

## Start the host and pair Android

1. Run `./result/bin/surveillance init`. It creates a configuration file and prints its path. Run `./result/bin/surveillance devices` to list available capture inputs.
2. On macOS, launch `open result/Applications/Surveillance.app`. On Linux, run `./result/bin/surveillance serve` in a terminal. The host listens on loopback port 8765. It writes operational messages to `host.log` alongside its configuration. To stop the macOS host, quit Surveillance in Activity Monitor.
3. Set up [Tailscale Serve](https://tailscale.com/docs/features/tailscale-serve) on the host and connect the phone to the same private network. Publish the loopback listener with `tailscale serve --bg http://127.0.0.1:8765`. Use the resulting HTTPS address in the Android app.
4. Install `android/app/build/outputs/apk/debug/app-debug.apk`. Run `./result/bin/surveillance pair` on the host and enter the HTTPS address and the one-time code in Android. The host supports one paired phone. To replace it, disconnect from the app or run `./result/bin/surveillance revoke` on the host.
5. Tap **Arm** in Android. macOS may ask for camera and microphone access. Allow access on the host and check that both sensors show **Ready**. Tap **Disarm** to release the inputs.

While armed, tap **Live view** to see the camera. Open a detection event to see its photo or tap **Play audio** to hear the clip. The camera preview and new event photos use compressed 320 × 240 JPEG images. The phone fetches media over private HTTPS.

The host needs to be running and awake. For Android alerts while the phone is locked or away from Wi-Fi, [configure Firebase and run the device checks](docs/setup.md#configure-firebase-alerts). Pairing, status, and event history also work with the build that omits Firebase configuration.

## Repository layout

- `crates/detect` contains the platform-independent motion and sound detectors.
- `crates/host` owns capture, the loopback HTTP API, the SQLite database, and FCM delivery.
- `android` contains the Kotlin and Jetpack Compose app.
- [docs/setup.md](docs/setup.md) covers configuration, notification delivery, and acceptance checks.
- [docs/api.md](docs/api.md) documents the phone API.
