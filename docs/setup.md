# Device setup

## Camera and microphone

Run `surveillance init` to find the configuration file. Restart the host after editing it.

| OS | Camera input | Microphone input |
| --- | --- | --- |
| macOS | System default camera or AVFoundation camera index | System default microphone or AVFoundation audio index |
| Linux | V4L2 device path, default `/dev/video0` | PulseAudio source, default `default`. PipeWire systems can use their PulseAudio-compatible server. |
| Windows | DirectShow video device name | DirectShow audio device name |

Set `camera_device` and `microphone_device` to the desired inputs from `surveillance devices`. On Windows set both names explicitly. On Linux and Windows the capture process runs with the permissions of the desktop user. On macOS, launch the packaged app from the logged-in session. If access was denied, grant it in System Settings under **Privacy & Security**, then restart the app if macOS requests it. A stalled or disconnected input retries independently.

`motion_region` is written as `[left, top, right, bottom]` in the 160 × 120 detection frame. Lower `motion_threshold` to respond to changes covering a smaller fraction of the region. Lower `sound_margin_db` to react to sounds closer to the background level. Both detectors group sustained activity into one event.

The API binds to loopback. Use a private Tailscale Serve endpoint with access scoped to your own devices. A paired phone can send arm/disarm commands with its device credential, which Android stores using Android Keystore. Disconnecting a phone revokes its access. Monitoring continues in the host's current armed state.

## Start at macOS login

Run `./packaging/install-macos-startup.sh` from the repository. It builds the signed app into a Nix store link in Surveillance's application data directory and registers a user LaunchAgent. The agent opens Surveillance at login and also starts Tailscale if the app is installed in Applications and its process is absent. Configure Tailscale and Serve once as described in the [README](../README.md). The macOS session must be open for this login agent to run.

Surveillance starts disarmed after every host launch. Use **Arm** in Android to start the camera and microphone. Closing the app on the phone or locking the phone leaves an already armed host running.

To turn off automatic startup, unload the agent with `launchctl bootout "gui/$(id -u)/org.surveillance.host.startup"` and remove `~/Library/LaunchAgents/org.surveillance.host.startup.plist`. You can still launch the host manually.

## Configure Firebase alerts

1. In your Firebase project, add an Android application with package name `org.surveillance.app`. Download `google-services.json` to `android/app/google-services.json`. Build and install the APK again from the Nix Android shell.
2. Enable the Firebase Cloud Messaging API. Use a Google Cloud service account that can send messages for this project. Store the service account JSON outside the repository with private file permissions. Set `firebase_credentials_file` in the host configuration to the absolute path and restart the host.
3. Pair the phone, then tap **Enable notifications** in the Android app if prompted. Allow the app's **Surveillance alerts** notification channel. The app registers its FCM token with the host.
4. Run `surveillance test-alert`. Even while disarmed, this writes a test event and queues an alert if the phone has registered a push token. Lock the phone, switch it to cellular, and repeat. Tap the notification to open the test event.

FCM requires Google Play services on the handset. Alerts carry a generic title and an event ID. The phone loads the camera image and audio clip directly from the host when you open the event. The phone opens the event over the private HTTPS connection. Queued alerts expire after two minutes. History stays on the host and catches up when the app refreshes after reconnecting.

## Acceptance checks

- On macOS, arm from Android and confirm both sensors reach **Ready**. Walk across the scene and clap. Check that two events appear and each notification opens the right event.
- Tap **Live view** while armed and check that the image updates. Open a fresh event to check its photo and play the audio.
- Switch on a light without moving through the scene and run a steady fan. Check for false positives and tune the two thresholds as needed.
- Disable phone connectivity, trigger an event, wait more than two minutes, and reconnect. Check that the event appears in history without receiving a stale alert.
- Unplug an external sensor while armed, check that the app reports the failure, then reconnect it. Disarm and check that the sensors return to **Idle**.
- Repeat real-device capture and reconnection on Linux and Windows before claiming hardware support for either platform.
