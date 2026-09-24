# SPEC-0009 — UI/UX

**Status:** Normative

The official identity is the supplied NeuralIA mark: white neural symbol on a black rounded square. The product UI is monochrome, spacious, text-first, and deliberately avoids dashboard clutter.

## Home

The Home surface contains the logo, product name/tagline, one native Windows omnibox, one primary action, explicit IA/Reader/Web actions, concise grammar help, and local status text.

The omnibox MUST be a platform text-edit control rather than a painted text editor. Selection, caret movement, clipboard operations, IME input, Home/End/Delete, and ordinary text-edit shortcuts therefore follow Windows behavior.

## Reader

Reader prioritizes comfortable line length, typography, semantic headings/lists, quotes, and whitespace-preserving code. It always exposes Home and Open full page.

## Keyboard

- Enter submits the omnibox.
Keyboard focus always lives in a child window (the native omnibox or a WebView2),
so the winit window never receives key events. Every shortcut MUST therefore be
delivered through one of two channels: the omnibox subclass (Home) or the keymap
injected into every page in the capture phase (Reader, Full Web, comparator),
which routes through the `neuralia:` action scheme. A shortcut that exists in
only one channel is a bug, not a limitation.

- Ctrl+L returns to the omnibox with its text selected.
- Ctrl+H opens the recent local-history viewer.
- Ctrl+Shift+Delete clears local history.
- Escape goes back one level: fullscreen column → three columns → Home.
- Backspace / Alt+Left / Alt+Right walk the page history.
- Ctrl+R and F5 reload; Ctrl + / - / 0 zoom on Chrome's ladder; Ctrl+F opens an
  in-page find bar; Ctrl+P prints; F12 opens DevTools; Ctrl+U shows the source.
- Ctrl + mouse wheel, and the precision-touchpad pinch that Chromium delivers as
  ctrl+wheel, step the same zoom ladder through the same `zoomin`/`zoomout`
  actions (one notch = one step; small pinch deltas add up to one). A page that
  handles the gesture itself (`preventDefault`) keeps it. WebView2's own zoom
  controls stay off, so nothing zooms twice. Gate:
  `ctrl_wheel_and_touchpad_pinch_zoom_through_the_app_steps` (runs the shipped
  keymap under Node). The pinch path was not exercised on touchpad hardware.
- F11 toggles fullscreen for the current comparator column; F8 toggles auto-scroll;
  1/2/3 expand a column and 0 restores.
- Backspace and the digits are ignored while typing in a field.

## Window chrome and the right-hand panels

- Home: the minimize/maximize/close buttons are not painted and do not take
  clicks until the pointer comes within 24 px of them; they hide again 300 ms
  after it leaves. In the comparator they are part of the bar and always shown.
  Gates: `caption_buttons_appear_near_the_cursor_and_hide_300_ms_after_it_leaves`,
  `caption_hot_zone_is_the_buttons_plus_the_margin_and_nothing_else`
  (`panel_chrome.rs`) and
  `home_window_buttons_show_only_once_revealed_and_the_bar_keeps_them`.
- The `−` and `⛶ <AI>` controls injected into each column show the app's
  centered hint ("Minimizar <AI>", "Expandir <AI>") through the closed `hint`
  action (SPEC-0005).
- The right-hand panel (Ctrl+H history, services, Gemini Live) is resized by
  dragging its left edge: between 300 px and 60% of the window, stored per
  panel kind in `<data_dir>/panel-width.json` (atomic write) and the AI
  columns reflow to the new edge. Gates:
  `panel_width_is_clamped_to_300_and_60_percent_of_the_window`,
  `panel_widths_survive_a_restart_and_a_broken_file_falls_back`,
  `the_resize_handle_sits_on_the_left_edge_away_from_the_scrollbar` and
  `the_columns_reflow_to_the_panel_edge_and_reclaim_it_when_minimized`.
- The mouse wheel over a visible right-hand panel scrolls the panel even when
  the keyboard focus is in an AI column. The routing decision is gated
  (`the_wheel_over_the_open_panel_goes_to_the_panel_and_nothing_else_is_touched`);
  its delivery, a low-level mouse hook that exists only while a panel is
  visible and acts only with NeuralIA in the foreground, has no automated test
  and was not observed on hardware.
- Service panels (Meet, WhatsApp, YouTube, Gmail) have a native strip in the
  comparator: Minimizar hides the panel while the page keeps running, the
  columns take the width back and the service icon gets a dot (red while the
  page plays sound); clicking the icon restores it. Tela cheia, or the page's
  own fullscreen (e.g. YouTube's button), fills the window; Esc or the page's
  exit returns. Gates: `service_panel_minimize_fullscreen_and_close_follow_one_state_machine`,
  `each_service_mode_gives_the_window_one_consistent_frame`,
  `the_strip_buttons_are_hit_where_they_are_drawn` and
  `service_panel_webview_signals_reach_only_the_panel_that_sent_them`. That
  audio keeps playing while the panel is hidden rests on WebView2 treating a
  hidden controller like a background tab; it was not observed on hardware.
- NeuralIA's own panel pages draw a thin, theme-coloured scrollbar instead of
  the classic Windows one (`the_history_panel_scrollbar_is_thin_and_follows_the_theme`
  checks the shipped stylesheet, not pixels). Third-party service pages keep
  their own scrollbars: nothing is injected into them.

## Auto-scroll

Auto-scroll is opt-in per session: opening a document asks Sim/Não at the bottom
centre, and nothing moves without an answer (an ignored prompt counts as Não).
The interval is 30 seconds from the first advance. A single document is advanced
with a synthesized Page Down so that HTML, plain text and the Edge PDF viewer all
behave the same; the synthesized key is only sent while the NeuralIA window is in
the foreground.

Controls require visible text/accessible labels; essential state must not rely on color alone.
