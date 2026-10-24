---
"task-server" = "minor"
"org-proto" = "minor"
"live-proto" = "none"
---

Added app documents: every org (your personal org included) now keeps a catalog of documents that client apps such as Studio create, list, rename and delete, and syncs their content to every device. A document can be made offline and registers when the device reconnects; a delete reaches devices that were away. The server stores them in `<org>/app_docs.sqlite` and never reads what is inside.
