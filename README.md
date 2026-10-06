# CCometixLine

[English](README.md) | [中文](README.zh.md)

A high-performance Claude Code statusline tool written in Rust with Git integration, usage tracking, interactive TUI configuration, and Claude Code enhancement utilities.

![Language:Rust](https://img.shields.io/static/v1?label=Language&message=Rust&color=orange&style=flat-square)
![License:MIT](https://img.shields.io/static/v1?label=License&message=MIT&color=blue&style=flat-square)

## Screenshots

![CCometixLine](assets/img1.png)

The statusline shows: Model | Directory | Git Branch Status | Context Window Information

## Features

### Core Functionality
- **Git integration** with branch, status, and tracking info  
- **Model display** with simplified Claude model names
- **Usage tracking** based on transcript analysis
- **Directory display** showing current workspace
- **Minimal design** using Nerd Font icons

### Interactive TUI Features
- **Interactive main menu** when executed without input
- **TUI configuration interface** with real-time preview
- **Theme system** with multiple built-in presets
- **Segment customization** with granular control
- **Configuration management** (init, check, edit)

### Claude Code Enhancement
- **Context warning disabler** - Remove annoying "Context low" messages
- **Verbose mode enabler** - Enhanced output detail
- **Robust patcher** - Survives Claude Code version updates
- **Automatic backups** - Safe modification with easy recovery

## Installation

### Quick Install (Recommended)

Install via npm (works on all platforms):

```bash
# Install globally
npm install -g @cometix/ccline

# Or using yarn
yarn global add @cometix/ccline

# Or using pnpm
pnpm add -g @cometix/ccline
```

Use npm mirror for faster download:
```bash
npm install -g @cometix/ccline --registry https://registry.npmmirror.com
```

After installation:
- ✅ Global command `ccline` is available everywhere
- ⚙️ Follow the configuration steps below to integrate with Claude Code
- 🎨 Run `ccline -c` to open configuration panel for theme selection

### Claude Code Configuration

Add to your Claude Code `settings.json`:

**Cross-Platform (Recommended)**
```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.claude/ccline/ccline",
    "padding": 0
  }
}
```

> **Note for Windows users:** Starting from Claude Code v2.1.47+, Unix-style path parsing is supported on Windows. The `~` symbol is automatically expanded to your user home directory. **Do not use `%USERPROFILE%`** - it no longer works reliably in v2.1.47+.
> - Recommended: `~/.claude/ccline/ccline` (works on all platforms)
> - Alternative: `"ccline"` (requires npm global installation)

**Fallback (npm installation):**
```json
{
  "statusLine": {
    "type": "command",
    "command": "ccline",
    "padding": 0
  }
}
```
*Use this if npm global installation is available in PATH*

### Update

```bash
npm update -g @cometix/ccline
```

<details>
<summary>Manual Installation (Click to expand)</summary>

Alternatively, download from [Releases](https://github.com/Haleclipse/CCometixLine/releases):

#### Linux

#### Option 1: Dynamic Binary (Recommended)
```bash
mkdir -p ~/.claude/ccline
wget https://github.com/Haleclipse/CCometixLine/releases/latest/download/ccline-linux-x64.tar.gz
tar -xzf ccline-linux-x64.tar.gz
cp ccline ~/.claude/ccline/
chmod +x ~/.claude/ccline/ccline
```
*Requires: Ubuntu 22.04+, CentOS 9+, Debian 11+, RHEL 9+ (glibc 2.35+)*

#### Option 2: Static Binary (Universal Compatibility)
```bash
mkdir -p ~/.claude/ccline
wget https://github.com/Haleclipse/CCometixLine/releases/latest/download/ccline-linux-x64-static.tar.gz
tar -xzf ccline-linux-x64-static.tar.gz
cp ccline ~/.claude/ccline/
chmod +x ~/.claude/ccline/ccline
```
*Works on any Linux distribution (static, no dependencies)*

#### macOS (Intel)

```bash  
mkdir -p ~/.claude/ccline
wget https://github.com/Haleclipse/CCometixLine/releases/latest/download/ccline-macos-x64.tar.gz
tar -xzf ccline-macos-x64.tar.gz
cp ccline ~/.claude/ccline/
chmod +x ~/.claude/ccline/ccline
```

#### macOS (Apple Silicon)

```bash
mkdir -p ~/.claude/ccline  
wget https://github.com/Haleclipse/CCometixLine/releases/latest/download/ccline-macos-arm64.tar.gz
tar -xzf ccline-macos-arm64.tar.gz
cp ccline ~/.claude/ccline/
chmod +x ~/.claude/ccline/ccline
```

#### Windows

```powershell
# Create directory and download
New-Item -ItemType Directory -Force -Path "$env:USERPROFILE\.claude\ccline"
Invoke-WebRequest -Uri "https://github.com/Haleclipse/CCometixLine/releases/latest/download/ccline-windows-x64.zip" -OutFile "ccline-windows-x64.zip"
Expand-Archive -Path "ccline-windows-x64.zip" -DestinationPath "."
Move-Item "ccline.exe" "$env:USERPROFILE\.claude\ccline\"
```

</details>

### Build from Source

```bash
git clone https://github.com/Haleclipse/CCometixLine.git
cd CCometixLine
cargo build --release

# Linux/macOS
mkdir -p ~/.claude/ccline
cp target/release/ccometixline ~/.claude/ccline/ccline
chmod +x ~/.claude/ccline/ccline

# Windows (PowerShell)
New-Item -ItemType Directory -Force -Path "$env:USERPROFILE\.claude\ccline"
copy target\release\ccometixline.exe "$env:USERPROFILE\.claude\ccline\ccline.exe"
```

## Usage

### Theme Override

```bash
# Temporarily use specific theme (overrides config file)
ccline --theme cometix
ccline --theme minimal
ccline --theme gruvbox
ccline --theme nord
ccline --theme powerline-dark

# Or use custom theme files from ~/.claude/ccline/themes/
ccline --theme my-custom-theme
```

### Claude Code Enhancement

```bash
# Disable context warnings and enable verbose mode
ccline --patch /path/to/claude-code/cli.js

# Example for common installation
ccline --patch ~/.local/share/fnm/node-versions/v24.4.1/installation/lib/node_modules/@anthropic-ai/claude-code/cli.js
```

## Default Segments

Displays: `Directory | Git Branch Status | Model | Context Window`

### Git Status Indicators

- Branch name with Nerd Font icon
- Status: `✓` Clean, `●` Dirty, `⚠` Conflicts  
- Remote tracking: `↑n` Ahead, `↓n` Behind

### Model Display

Shows simplified Claude model names:
- `claude-3-5-sonnet` → `Sonnet 3.5`
- `claude-4-sonnet` → `Sonnet 4`

When Claude Code reports them, the reasoning effort level and fast mode follow the name, as in `Opus 5.5 · high · fast`. Either can be hidden, see [Segment Options](#segment-options).

### Context Window Display

Tokens in the context window and their share of the context limit, from the usage Claude Code reports after each response. A new session shows `-` until its first response, and so does a session after `/compact`.

### Usage Display

For Claude subscriptions: the 5-hour limit's usage and when it resets, then the 7-day limit's usage, as in `24% · 14:00 · 7d 41%`. Options can add when the 7-day limit resets and the usage of the weekly Fable limit, as in `24% · 14:00 · 7d 41% · 10-08 14:00 · Fable 12%`.

### Prompt Cache Display

The prompt cache hit ratio Claude Code reports, as in `92% · 14:32`: the share of the main conversation's input tokens read from the cache. Claude Code counts from when it started, so resuming a session starts the count over, and subagents are not included. For Claude models it adds when the cache expires, or `cold` after it has. Claude Code works that out from Anthropic's cache lifetimes, so other providers' models show only the share. Disabled by default; enable it with `ccline --config`.

### Line Wrapping

When the segments do not fit, the status line continues on the next line, breaking between segments. Claude Code gives the status line the terminal width minus 4 columns and the `statusLine.padding` setting on both sides. ccline reads the padding in Claude Code's order of precedence: the project's `.claude/settings.local.json` and `.claude/settings.json`, then `settings.json` in `CLAUDE_CONFIG_DIR` (`~/.claude` by default). Managed settings are not read.

## Configuration

CCometixLine supports full configuration via TOML files and interactive TUI:

- **Configuration file**: `~/.claude/ccline/config.toml`
- **Interactive TUI**: `ccline --config` for real-time editing with preview
- **Theme files**: `~/.claude/ccline/themes/*.toml` for custom themes
- **Automatic initialization**: `ccline --init` creates default configuration

### Available Segments

All segments are configurable with:
- Enable/disable toggle
- Custom separators and icons
- Color customization
- Format options

Supported segments: Model, Directory, Git, Context Window, Usage, Cost, Prompt Cache, Session, Output Style

### Segment Options

In `ccline --config`, select a segment and press Tab: its options follow Text Style in the settings panel. Enter switches an on/off option, or opens an input box for other values, where an empty input restores the default. `config.toml` keeps them under the segment's `[segments.options]`.

| Segment | Option | Key | Default |
|---|---|---|---|
| Model | Effort level | `show_effort` | on |
| Model | Fast mode | `show_fast_mode` | on |
| Git | Commit SHA | `show_sha` | off |
| Usage | Reset time of the 5-hour limit | `show_reset_time` | on |
| Usage | Usage of the 7-day limit | `show_seven_day` | on |
| Usage | Reset time of the 7-day limit, shown after its usage | `show_seven_day_reset` | off |
| Usage | Usage of the weekly Fable limit | `show_fable` | off |
| Usage | API base URL | `api_base_url` | `https://api.anthropic.com` |
| Usage | API cache duration, in seconds | `cache_duration` | 180 |
| Usage | API timeout, in seconds | `timeout` | 2 |
| Prompt Cache | Expiry time | `show_expiry` | on |

The Usage segment queries the usage API only when Claude Code does not report rate limits: before a session's first response, or with older Claude Code versions. Claude Code does not report the Fable limit, so with `show_fable` on, the segment also queries the API and shows the other limits from Claude Code. Sessions with the same `CLAUDE_CONFIG_DIR`, and so the same account, share a cache: the API is queried at most once per `cache_duration` for them, even when a request fails or several sessions refresh at the same moment. Once the API has not answered for three `cache_duration`s, its last answer is no longer shown. After you sign out of Claude Code or switch to an API key, the cached answer is cleared the next time the API is due.

### Model Configuration (`models.toml`)

Location: `~/.claude/ccline/models.toml` (auto-created on first run)

This file configures model display names, context windows and prices. Claude models (Sonnet, Opus, Haiku) are automatically recognized with version extraction. Common third-party models (DeepSeek, GLM, Kimi, Qwen, MiniMax) are built in with their context windows and China-platform prices in CNY, see [`builtin_models.toml`](src/config/builtin_models.toml). Add entries here to override them (for example with USD prices from an international platform) or to add other models. Earlier versions also had built-in substring entries for `glm-4.5`, `kimi-k2` and `qwen3-coder` (128k to 256k windows); if you still use these models, add entries with their `context_limit`. Check Configuration in the `ccline` menu reports a `models.toml` that does not parse, which the status line otherwise ignores.

- **Display name**: models without a configured name show Claude Code's model ID without tags such as `[1m]`.
- **Context window**: the context segment uses the smaller of the model's `context_limit` and the window Claude Code works with. For a model ID it does not know, Claude Code uses 1M when the ID carries `[1m]`, otherwise the `CLAUDE_CODE_MAX_CONTEXT_TOKENS` environment variable, or 200k when that is unset. Claude Code compacts the conversation based on this window, so it should match the model's real window. Unlike in earlier versions, `[1m]` does not override `context_limit`. Claude models always use the window Claude Code reports.
- **Cost**: Claude Code prices models it does not know at Claude Opus rates. For models with `pricing`, the cost segment sums the session's transcripts (subagents and workflow agents included) at those prices instead. Responses from models without a price in the current model's currency are left out, and the cost then ends with `+`, as in `¥1.20+`. Requests Claude Code does not record in transcripts, such as session title generation, are not counted.
- **Off-peak prices**: each request is priced at the time it was made. Public holidays are not tracked, so peak-hour requests on a weekday holiday are priced at peak rates.

```toml
# Entries match the model ID by substring, or exactly with match = "exact".
# A substring also matches longer IDs: "glm-5.3" matches "glm-5.3-flash" too,
# so override a single built-in model with match = "exact".
# They take priority over built-in entries; fields left out fall back to the next match.
[[models]]
pattern = "my-model"
display_name = "My Model"
context_limit = 128000

# Rates per 1M tokens
[models.pricing]
currency = "$"
input = 0.3
output = 1.2
cache_read = 0.03       # defaults to input
cache_write = 0.3       # 5-minute cache writes, defaults to input
cache_write_1h = 0.6    # defaults to cache_write

# Higher rates once a request's input tokens (including cache) reach min_input.
# Cache prices left out here are the ones above.
[[models.pricing.tiers]]
min_input = 32000
input = 0.6
output = 2.4

# Discount outside peak hours
[models.pricing.off_peak]
multiplier = 0.5
utc_offset = 8          # hours east of UTC that peak_hours are in, such as 5.5
peak_hours = ["09:00-12:00", "14:00-18:00"]
weekdays_only = true

# Context declarations inside model IDs, used when Claude Code does not report
# the context window itself. display_suffix optionally appends text to the name.
[[context_modifiers]]
pattern = "[1m]"
display_suffix = " 1M"
context_limit = 1000000
```


## Requirements

- **Git**: Version 1.5+ (Git 2.22+ recommended for better branch detection)
- **Terminal**: Must support Nerd Fonts for proper icon display
  - Install a [Nerd Font](https://www.nerdfonts.com/) (e.g., FiraCode Nerd Font, JetBrains Mono Nerd Font)
  - Configure your terminal to use the Nerd Font
- **Claude Code**: For statusline integration

## Development

```bash
# Build development version
cargo build

# Run tests
cargo test

# Build optimized release
cargo build --release
```

## Roadmap

- [x] TOML configuration file support
- [x] TUI configuration interface
- [x] Custom themes
- [x] Interactive main menu
- [x] Claude Code enhancement tools

## Contributing

Contributions are welcome! Please feel free to submit issues or pull requests.

## Related Projects

- [tweakcc](https://github.com/Piebald-AI/tweakcc) - Command-line tool to customize your Claude Code themes, thinking verbs, and more.

## License

This project is licensed under the [MIT License](LICENSE).

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=Haleclipse/CCometixLine&type=Date)](https://star-history.com/#Haleclipse/CCometixLine&Date)
