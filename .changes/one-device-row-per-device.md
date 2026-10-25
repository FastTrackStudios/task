---
"task-server" = "minor"
"task-node-host" = "patch"
"ui" = "patch"
"live-proto" = "none"
---

Fixed a device appearing more than once in your account's device list: Task now registers under the id every FastTrackStudio app on the device derives (so Task and Studio on one iPad or Mac are one device), and takes this device's older rows (its previous random id, or the same name and platform under another id) off the account once. iPads are no longer named "iPhone". Architect moves to v0.9.30.
