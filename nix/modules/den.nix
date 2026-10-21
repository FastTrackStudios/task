# Den wiring — the dendritic spine of this flake.
#
# import-tree (flake.nix) loads every .nix file under nix/modules/ as a
# flake-parts module; den's flakeModule adds the aspect system on top so
# features can be published as aspects and consumed from the system
# flake (and vice versa) without copy-paste.
#
# Today the heavy outputs (shells, crane packages, images) are ordinary
# perSystem modules speaking the `fts.*` schema (options.nix). The
# headless node's systemd service is exported as a plain NixOS module,
# `nixosModules.task-server` (nixos-modules.nix, over
# nix/nixos/task-server.nix), so any NixOS flake can import it; a den
# fleet wraps it in one line (`den.aspects.task-node.nixos =
# inputs.task.nixosModules.task-server;`). New shareable features belong
# here as den.aspects.<name>.
{ inputs, ... }:
{
  imports = [ inputs.den.flakeModule ];
}
