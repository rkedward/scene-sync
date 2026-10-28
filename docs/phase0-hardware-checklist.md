# Phase 0: projector checklist

Run these on one HY300 Pro+ before relying on it. Write results next to each item.

## Setup

- [ ] Enable developer options (Settings > About, press the build/version entry 7 times) and network or USB debugging
- [ ] `adb connect <ip>:5555` works from a laptop
- [ ] Debug APK installs with `adb install -r`

## Playback

- [ ] SceneSync Player pairs and loops a 720p H.264 clip smoothly for 1 hour
- [ ] Same clip at 1080p: smooth or stuttering?
- [ ] Audio plays from the built-in speaker
- [ ] Note the time from "Play" in the dashboard to picture on the wall

## Staying on

- [ ] Set SceneSync Player as the home app. Reboot from settings: does it come back by itself?
- [ ] Pull the power plug and restore it: does the projector turn on by itself? (Needed for smart-plug scheduling.)
- [ ] After power-on, does SceneSync come back and resume the right clip?
- [ ] Leave it idle (black) for 2 hours: any screensaver, sleep, or "no signal" screen?
- [ ] Remote keys: does any key exit the player unexpectedly?

## Picture

- [ ] In a dark room, how visible is the "black" rectangle? (Matters for window illusions.)
- [ ] Reset keystone to neutral before any fit-to-window work

## Network and time

- [ ] Player finds the server automatically (mDNS), or needs the address typed in
- [ ] Unplug the router: the clip keeps playing
- [ ] With the router back but no Internet, does the projector's clock drift? Compare to a phone after 24 hours.

## Storage

- [ ] Free space reported in the dashboard matches Settings (about 2.4 GB)
