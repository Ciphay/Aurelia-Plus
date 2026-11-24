# examples

Examples of how a launcher uses the `aurelia bridge serve` contract (`docs/overlay-json.md`). Nothing here is required for the shim to work.

- `overlay/`: reference overlay in Electron (a window on top of the game) and the `vklayer/` Vulkan layer that draws it inside the game's frames.
  It only starts with `AURELIA_PLUS_OVERLAY=1` or `ingame` in `launch.sh`; without that, `launch.sh` does not start any overlay.
- To develop a launcher without opening a game: `aurelia --json bridge simulate` emits sample events and answers commands from stdin.
