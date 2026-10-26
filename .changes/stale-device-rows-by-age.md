---
"task-server" = "patch"
---

Fixed two devices with the same name (two iPads both called "Cody's iPad") taking each other off the account's device list on every sync: a same-named row is now retired as this device's leftover only once nobody has refreshed it for a day, and servers never match by name.
