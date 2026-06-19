# mgmt.nvim

Frontmatter completion for [mgmt](../../) task markdown files. When you hand-edit a task's YAML
frontmatter, it suggests the available fields and their valid values — status ids, known
projects, areas, tags — sourced live from `mgmt meta --json` so they always match your config.

It is an [nvim-cmp](https://github.com/hrsh7th/nvim-cmp) source. **If the `mgmt` binary is not on
your `PATH`, the plugin loads inertly and registers nothing — no errors, no completions.**

## What it completes

Inside the leading `---`…`---` frontmatter of a task file (detected by living under your vault's
`tasks/` dir, or by having a `uid:` line):

- **Keys**: `title`, `status`, `priority`, `project`, `area`, `tags`, `due`, `scheduled`,
  `reminders`.
- **Values** after a `key:`:
  - `status:` → your configured status ids (e.g. `todo`, `doing`, `blocked`, `done`)
  - `priority:` → `none` / `low` / `medium` / `high`
  - `project:` → projects known to mgmt
  - `area:` / `tags:` → values already used across your vault
  - `reminders:` → example offsets (`1d`, `2h`, `30m`)

## Install (generic, nvim-cmp)

With any plugin manager, point it at this directory, then register the source:

```lua
require("mgmt").setup()  -- registers the "mgmt" cmp source (no-op if mgmt isn't installed)

require("cmp").setup({
  sources = {
    { name = "mgmt" },
    -- … your other sources …
  },
})
```

`setup()` is safe to call unconditionally. The cached schema refreshes on `:w` of any markdown
buffer (so newly-added projects/statuses show up without restarting nvim).

## Install (Nix / lazy.nvim)

This repo is wired into the NixOS config: it is added to the lazy.nvim store path and the cmp
`sources` list. See `nixos/vim/plugins.nix` and `nixos/vim/lazy-spec.nix` in the user's nixos
config for the exact entries.
