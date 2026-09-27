# Camera reports

What works with DragonSlayer, model by model. Add yours with the *Camera report* issue template.

Legend: ✅ works · ⚠️ works with caveats (see notes) · ❌ doesn't work · ❔ not tested yet

| Make | Model | OS | Connects | Live view | Capture | Download | RAW+JPEG | DragonSlayer version | Notes |
|---|---|---|---|---|---|---|---|---|---|
| Panasonic | Lumix GH5 | Windows | ✅ | ⚠️ | ✅ | ✅ | ❔ | 0.2.0-beta | Reference camera. USB Mode → PC(Tether). Live view can lock up the camera over long sessions; DragonSlayer pauses it in Preview mode to give the camera a rest. Power-cycling the camera clears it. |
| Panasonic | Lumix GH5 | macOS | ✅ | ❔ | ✅ | ✅ | ❔ | 0.2.1-beta | Reference camera. USB Mode → PC(Tether). Tested with the packaged app (capture + download + compile). A timeout after the app quits mid-session is cleared by power-cycling the camera. |
| Canon | EOS 100D | Windows | ✅ | ✅ | ✅ | ✅ | ✅ | 0.3.0-beta | Thoroughly tested. Mode dial on **M** and Live View shooting enabled in the menu. Zadig's WinUSB swap is per USB port: on a new port Windows puts its own driver back (Diagnose camera spots this). PTP timeouts through an unpowered USB hub went away when plugged straight into the computer. The camera shows a computer icon while tethered; that's normal. |
