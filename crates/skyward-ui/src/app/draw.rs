//! Drawing: the page rendered a band at a time, and what the panel shows
//! composed from it, with the bar, the titles and what slides or fades
//! over the page.

use super::*;
use crate::icons::{self, Icon};

impl App {
    /// What the status bar shows, its clock reading `clock`: the battery,
    /// the network joined, the data partition's lock, the slot the system
    /// runs from, and the battery's temperature.
    pub fn bar<'a>(&self, clock: &'a str, info: &DeviceInfo) -> Bar<'a> {
        let battery = info.battery.as_ref();
        Bar {
            clock,
            battery: battery.map(|b| (b.level, b.charging().then_some(if b.quick { Bolt::Quick } else { Bolt::Charging }))),
            wlan: self.wlan_mark(),
            data: match self.state.data {
                Data::Erased => None,
                Data::Open => Some(true),
                Data::Closed | Data::Opening | Data::Failed(_) | Data::Locked { .. } => Some(false),
            },
            slot: info.slot.map(Slot::letter),
            temp: battery.and_then(|b| b.temp),
        }
    }

    /// Render the current page if what it shows has changed: the rows of
    /// it on screen, the rest as they come into view; or, where only some
    /// rows changed, those rows.
    pub fn frame(&mut self, f: &Fonts, info: &DeviceInfo) -> &Frame {
        self.render(f, info, self.w)
    }

    /// [`App::frame`], drawing of a page rendered anew only its columns
    /// from the left edge to `wide`.
    fn render(&mut self, f: &Fonts, info: &DeviceInfo, wide: u32) -> &Frame {
        if self.frame.is_some() && !self.stale.is_empty() {
            let t = self.theme();
            let screen = self.screen();
            for rows in std::mem::take(&mut self.stale) {
                let band = Band { rows, cols: (0, self.cols) };
                let cx = Ctx { t: &t, f, info, state: &self.state, term: self.term.as_ref(), band: Some(band) };
                let fresh = pages::render(screen, &cx);
                let mut canvas = self.frame.take().expect("checked above").canvas;
                assert_eq!(canvas.h, fresh.canvas.h, "a page drawn again in part is as tall as it was");
                canvas.replace(&fresh.canvas);
                self.frame = Some(Frame { canvas, ..fresh });
            }
        }
        if self.frame.is_none() {
            let t = self.theme();
            let screen = self.screen();
            let first = self.top().1.max(0.0) as u32;
            let band = Band { rows: (first, first + self.h), cols: (0, wide.min(self.w)) };
            let cx = Ctx { t: &t, f, info, state: &self.state, term: self.term.as_ref(), band: Some(band) };
            let mut frame = pages::render(screen, &cx);
            // A page drawn in part across holds its rows whole, the columns
            // of them drawn on as they are needed.
            self.cols = frame.canvas.cols().end;
            if self.cols < self.w {
                let mut whole = Canvas::unfilled(self.w, frame.canvas.h, frame.canvas.held());
                whole.replace(&frame.canvas);
                frame.canvas = whole;
            }
            self.height = frame.height(&t);
            self.frame = Some(frame);
            self.clamp_scroll();
            if std::mem::take(&mut self.snap) {
                let (title, bar) = self.bar_wanted().expect("rendered above");
                self.title.jump(title);
                self.bar.jump(bar);
            }
        }
        self.frame.as_ref().expect("rendered above")
    }

    /// The current page rendered, all of it drawn.
    pub fn whole_frame(&mut self, f: &Fonts, info: &DeviceInfo) -> &Frame {
        let h = self.frame(f, info).canvas.h;
        self.draw_cols(f, info, self.w);
        self.draw_rows(f, info, 0.0, h as f32);
        self.frame.as_ref().expect("rendered above")
    }

    /// Draw a part of the current page not drawn yet: the rest of the
    /// columns of its rows drawn, then a band of rows below those or else
    /// above them, for a page at rest to have all of it drawn by the time
    /// it scrolls into view; that done, make the About page field's dither,
    /// for that page to open without making it. Whether any is left to do.
    pub fn draw_ahead(&mut self, f: &Fonts, info: &DeviceInfo) -> bool {
        let Some(frame) = &self.frame else { return false };
        let (held, h) = (frame.canvas.held(), frame.canvas.h);
        if self.cols < self.w {
            self.draw_cols(f, info, self.w);
        } else if held.end < h {
            self.draw_rows(f, info, held.end as f32, (held.end + 1) as f32);
        } else if held.start > 0 {
            self.draw_rows(f, info, (held.start - 1) as f32, held.start as f32);
        } else if self.dither.is_none() {
            self.dither = Some(glow::Dither::new(self.w as usize, self.h as usize));
        }
        self.cols < self.w || self.frame.as_ref().is_some_and(|fr| !fr.canvas.whole()) || self.dither.is_none()
    }

    /// Draw the columns of the current page's rows drawn from where they
    /// were drawn to, on to `wide`.
    fn draw_cols(&mut self, f: &Fonts, info: &DeviceInfo, wide: u32) {
        let wide = wide.min(self.w);
        let Some(frame) = &self.frame else { return };
        if self.cols >= wide {
            return;
        }
        let held = frame.canvas.held();
        let t = self.theme();
        let band = Band { rows: (held.start, held.end), cols: (self.cols, wide) };
        let cx = Ctx { t: &t, f, info, state: &self.state, term: self.term.as_ref(), band: Some(band) };
        let part = pages::render(self.screen(), &cx).canvas;
        self.frame.as_mut().expect("drawn above").canvas.replace(&part);
        self.cols = wide;
        self.layer = None;
    }

    /// Draw the rows `from..to` of the current page it has not drawn yet,
    /// a band at least a quarter of a screen tall at a time; the rows drawn
    /// are drawn across first.
    fn draw_rows(&mut self, f: &Fonts, info: &DeviceInfo, from: f32, to: f32) {
        let Some(frame) = &self.frame else { return };
        let h = frame.canvas.h;
        let (from, to) = ((from.max(0.0) as u32).min(h), (to.max(0.0).ceil() as u32).min(h));
        let held = frame.canvas.held();
        let least = self.h / 4;
        let mut bands = Vec::new();
        if from < held.start {
            bands.push((from.min(held.start.saturating_sub(least)), held.start));
        }
        if to > held.end {
            bands.push((held.end, to.max(held.end + least).min(h)));
        }
        if bands.is_empty() {
            return;
        }
        self.draw_cols(f, info, self.w);
        let t = self.theme();
        let screen = self.screen();
        for rows in bands {
            let band = Band { rows, cols: (0, self.w) };
            let cx = Ctx { t: &t, f, info, state: &self.state, term: self.term.as_ref(), band: Some(band) };
            let part = pages::render(screen, &cx).canvas;
            self.frame.as_mut().expect("drawn above").canvas.join(&part);
        }
        self.layer = None;
    }

    pub(super) fn max_scroll(&self) -> f32 {
        (self.height - self.h as f32).max(0.0)
    }

    fn clamp_scroll(&mut self) {
        let max = self.max_scroll();
        let top = self.top_mut();
        top.1 = top.1.clamp(0.0, max);
    }

    /// Draw what the panel shows now: the page, the charging screen over it
    /// while a cable has just gone in, both as dark as the content stands
    /// as the panel goes off or comes up.
    pub fn compose(&mut self, out: &mut Canvas, f: &Fonts, info: &DeviceInfo, clock: &str, now: u64) {
        let charge = self.charging.as_ref().map_or(0.0, |c| c.shown.level(now));
        if charge < 1.0 {
            self.compose_page(out, f, info, clock, now);
        }
        if charge > 0.0 {
            self.compose_charging(out, f, info, charge, now);
        }
        if self.shade > 0.0 {
            self.paint_shade(out, f, info, clock, now);
        }
        // The page comes up from black as the panel lights, and goes dark as
        // it goes off. The lock is drawn while the panel is lit, and through
        // the fade if the lock was what the panel was showing when it began to
        // go dark: the page under it is not what was on the screen, and showing
        // it for the moment the backlight takes to close reads as an unlock.
        let shown = self.display_on || self.lock_shown;
        if shown
            && let Some(lock) = &self.lock
        {
            let drag = lock.drag;
            self.compose_lock(out, f, info, clock, drag);
        }
        out.dim(self.curtain.level(now));
    }

    /// The control centre, pulled down from the top edge: the time and the
    /// date, the twelve round tiles in three rows of four, the card saying
    /// how the phone stands, and the two capsules.
    fn paint_shade(&mut self, out: &mut Canvas, f: &Fonts, info: &DeviceInfo, clock: &str, now: u64) {
        self.shade_hits.clear();
        if self.shade <= 0.0 {
            // Nothing of the sheet shows, so nothing of it rolls either.
            self.shade_roll = None;
            return;
        }
        let t = self.theme();
        let m = t.m;
        let w = m.w as f32;
        let l = self.state.lang;
        let white = Rgba::rgb(255, 255, 255);
        let tall = m.dp(SHADE_HEIGHT_DP);
        let sheet = tall * self.shade;
        let top = sheet - tall;
        let pane = Rgba::rgb(20, 20, 26).with_alpha(238);
        // What the WLAN tile says: the network joined where one is, and how the
        // WLAN stands otherwise. Kept as its own string, since the tile records
        // where the name has rolled to while it draws it.
        let wlan = self.state.wlan.as_ref().map_or_else(|| l.pick(["已关闭", "已關閉", "Off"]), |w| fmt::wlan_value(l, w)).to_owned();
        // The sheet itself, its foot rounded off.
        //
        // One shape, drawn once. It was a body, two circles and a strip between
        // them, and each of the four laid the pane at its own alpha: where they
        // met along the foot the corners came out laid twice, which is darker
        // than the rest, and the circle's antialiased edge left a wedge of the
        // page showing beside it. One rounded rect has no joins to show.
        //
        // The top corners are square, since the sheet comes down from the top
        // edge; only the foot is rounded, so the radius is given to the bottom
        // alone by drawing the shape a radius taller than it shows and letting
        // the screen's edge cut the top.
        //
        // A barely opened sheet is shorter than the radius, and a radius that
        // large would round its top corners too, leaving the screen's edge
        // showing through them. The radius gives way to the sheet's height.
        let radius = m.dp(30.0).min(sheet.max(0.0));
        out.fill_round_rect(0.0, top - radius, w, sheet - top + radius, radius, pane);
        // A grabber at the foot of the sheet.
        out.fill_round_rect(w / 2.0 - m.dp(18.0), sheet - m.dp(13.0), m.dp(36.0), m.dp(5.0), m.dp(2.5), white.with_alpha(80));
        // The time, with the date beside it on the same line: the date set a
        // size smaller and taking its width from the time, so the two read as
        // one line rather than two blocks. The battery's percentage used to sit
        // at the far right; it is on the card below now, beside the rest of
        // what the phone says of itself.
        let inset = m.dp(15.0);
        let time_px = m.dp(44.0);
        out.text(&f.heading, clock, m.margin + inset, top + m.dp(88.0), time_px, white, None);
        let date = self.date.as_str();
        if !date.is_empty() {
            let wide = out.text_width(&f.heading, clock, time_px);
            out.text(&f.body, date, m.margin + inset + wide + m.dp(12.0), top + m.dp(86.0), m.dp(19.0), white.with_alpha(170), None);
        }
        // The twelve round tiles, three rows of four. A tile is a disc with its
        // mark in it and its word under it, and the ones that turn something on
        // show lit while it is on.
        let (gap, disc, word_gap) = (m.dp(10.0), m.dp(56.0), m.dp(7.0));
        let cell = (w - m.margin * 2.0 - gap * 3.0) / 4.0;
        let tiles_top = top + m.dp(116.0);
        let row_h = disc + word_gap + m.dp(19.0);
        let mut hits = Vec::new();
        for (i, hit) in ShadeHit::TILES.into_iter().enumerate() {
            let (col, row) = (i % 4, i / 4);
            let cx = m.margin + col as f32 * (cell + gap) + cell / 2.0;
            let cy = tiles_top + row as f32 * (row_h + gap) + disc / 2.0;
            let lit = hit.lit(&self.state);
            // The whole cell answers a touch, not the disc alone: a finger is
            // not asked to find the middle of it.
            hits.push((Rect::new(cx - cell / 2.0, cy - disc / 2.0 - m.dp(2.0), cell, row_h + m.dp(4.0)), hit));
            let fill = if lit { t.pal.accent.with_alpha(232) } else { white.with_alpha(30) };
            out.fill_circle(cx, cy, disc / 2.0, fill);
            let ink = if lit { white } else { white.with_alpha(238) };
            Self::shade_glyph(out, &t, hit, cx, cy, m.dp(26.0), ink);
            // The tile's word, kept inside its cell however long it is: a name
            // wider than the cell would run under the tile beside it.
            let label = Self::shade_label(l, hit, &wlan);
            let px = widgets::fitted_px(out, &f.body, label, m.dp(SHADE_LABEL_DP), m.dp(SHADE_NAME_LEAST_DP), cell);
            let travel = out.text_width(&f.body, label, px);
            let baseline = cy + disc / 2.0 + word_gap + m.dp(14.0);
            if travel <= cell {
                if hit == ShadeHit::Wlan {
                    self.shade_roll = None;
                }
                let x = cx - travel / 2.0;
                out.text(&f.body, label, x, baseline, px, ink, None);
            } else if hit == ShadeHit::Wlan {
                // A name still too long at the least size rolls, rather than
                // being cut: the whole of it is read as it passes.
                let since = match self.shade_roll {
                    Some(r) if (r.travel - travel).abs() < 0.5 => r.since,
                    _ => now,
                };
                let roll = Roll { since, travel, room: cell, pace: roll_pace(m.dp(1.0)) };
                self.shade_roll = Some(roll);
                let x = cx - cell / 2.0;
                Self::paint_rolled(out, &f.body, label, x, baseline, px, cell, ink, roll.offset(now));
            } else {
                // Anything else too long is set from its middle and let past
                // its cell, since it names a page rather than a network.
                let x = cx - travel / 2.0;
                out.text(&f.body, label, x, baseline, px, ink, None);
            }
        }
        // The card saying how the phone stands: whether its data is open, the
        // slot it starts from, what is left in the battery and how warm it is.
        let card_top = tiles_top + 3.0 * (row_h + gap) + m.dp(4.0);
        let card = Rect::new(m.margin, card_top, w - m.margin * 2.0, m.dp(58.0));
        out.fill_round_rect(card.x, card.y, card.w, card.h, m.dp(18.0), white.with_alpha(20));
        let cells: [String; 4] = [
            fmt::data(l, &self.state.data).to_owned(),
            info.slot.map_or_else(|| "\u{2014}".to_owned(), |s| s.letter().to_owned()),
            info.battery.as_ref().map_or_else(|| "\u{2014}".to_owned(), |b| format!("{}%", b.level)),
            info.battery.as_ref().and_then(|b| b.temp).map_or_else(|| "\u{2014}".to_owned(), fmt::celsius),
        ];
        let caps = [
            l.pick(["数据", "資料", "Data"]),
            l.pick(["槽位", "槽位", "Slot"]),
            l.pick(["电量", "電量", "Battery"]),
            l.pick(["温度", "溫度", "Temp"]),
        ];
        let cell_w = card.w / 4.0;
        for (i, (value, cap)) in cells.iter().zip(caps).enumerate() {
            let cx = card.x + cell_w * i as f32 + cell_w / 2.0;
            let wide = out.text_width(&f.body, value, m.row_title_px);
            out.text(&f.body, value, cx - wide / 2.0, card.y + m.dp(26.0), m.row_title_px, white, None);
            let small = m.dp(12.0);
            let wide = out.text_width(&f.body, cap, small);
            out.text(&f.body, cap, cx - wide / 2.0, card.y + m.dp(45.0), small, white.with_alpha(150), None);
        }
        // The backlight and the motor: two capsules side by side, each filled
        // from its left end as far as its value goes, with the mark alone at
        // its middle -- the name inside was never level with anything. A finger
        // anywhere on one slides it.
        let cap_h = m.dp(46.0);
        let cap_w = (w - m.margin * 2.0 - gap) / 2.0;
        let row = card.y + card.h + m.dp(14.0);
        for (col, (which, hit)) in [(Slider::Brightness, ShadeHit::Brightness), (Slider::Vibration, ShadeHit::Vibration)].into_iter().enumerate() {
            let value = match which {
                Slider::Brightness => self.state.brightness,
                // The sheet's two are the only ones it holds; the charge
                // page's are drawn on the charge page.
                _ => self.state.vibration,
            }
            .clamp(0.0, 1.0);
            let cap = Rect::new(m.margin + col as f32 * (cap_w + gap), row, cap_w, cap_h);
            out.fill_round_rect(cap.x, cap.y, cap.w, cap.h, cap.h / 2.0, white.with_alpha(26));
            // The fill keeps the capsule's own ends: narrow, it is still a
            // capsule, not a bar with one round corner.
            let filled = (cap.w * value).max(cap.h);
            out.fill_round_rect(cap.x, cap.y, filled, cap.h, cap.h / 2.0, t.pal.accent.with_alpha(210));
            // The mark stands at the capsule's left end, level with its
            // middle, as a control's own icon does; the rest of the capsule is
            // the level it is set to.
            Self::shade_glyph(out, &t, hit, cap.x + m.dp(26.0), cap.y + cap.h / 2.0, m.dp(23.0), white.with_alpha(235));
            hits.push((cap, hit));
        }
        self.shade_hits = hits;
    }

    /// The WLAN tile's name, set from `x` at `px`, kept inside `room` however
    /// long it is: a name too long to fit is set smaller where that is enough,
    /// and rolled where it is not.
    ///
    /// The roll is the whole name drawn as ink and laid down shifted inside a
    /// strip the width of the tile, so no part of a name is ever cut by its
    /// bytes -- an SSID may be anything at all, and slicing one by index would
    /// split a character.
    #[allow(clippy::too_many_arguments)]
    fn paint_rolled(out: &mut Canvas, face: &crate::font::Face, name: &str, x: f32, baseline: f32, px: f32, room: f32, ink: Rgba, shift: f32) {
        let (_, top) = face.ink(px);
        // A sprite of the name as it stands shifted, in white, kept as
        // coverage alone: it is laid down in the tile's own ink.
        let strip = Sprite::capture(x, baseline + top, room.ceil() as u32, (px * 1.6).ceil() as u32, |c, dx, dy| {
            c.text(face, name, x - shift + dx, baseline + dy, px, Rgba::rgb(255, 255, 255), None);
        });
        strip.draw(out, 0.0, 0.0, ink, 1.0);
    }

    /// What a tile is called on the sheet: the network's own name for the
    /// WLAN, and a word for each of the others.
    fn shade_label(l: Lang, hit: ShadeHit, wlan: &str) -> &str {
        match hit {
            ShadeHit::Wlan => wlan,
            ShadeHit::Vibrate => l.pick(["震动", "震動", "Vibration"]),
            ShadeHit::Transfer => l.pick(["文件传输", "檔案傳輸", "Transfer"]),
            ShadeHit::UsbDrive => l.pick(["U 盘", "USB 隨身碟", "USB drive"]),
            ShadeHit::Files => l.pick(["文件管理", "檔案管理", "Files"]),
            ShadeHit::Modules => l.pick(["模块管理", "模組管理", "Modules"]),
            ShadeHit::Partitions => l.pick(["分区管理", "分割區管理", "Partitions"]),
            ShadeHit::Terminal => l.pick(["终端", "終端機", "Terminal"]),
            ShadeHit::Sleep => l.pick(["熄屏", "關閉螢幕", "Screen off"]),
            ShadeHit::Screenshot => l.pick(["截图", "截圖", "Screenshot"]),
            ShadeHit::Reboot => l.pick(["重启", "重新啟動", "Reboot"]),
            ShadeHit::Settings => l.pick(["设置", "設定", "Settings"]),
            ShadeHit::Brightness | ShadeHit::Vibration => "",
        }
    }

    /// The mark a tile or a slider carries.
    fn shade_glyph(out: &mut Canvas, t: &Theme, hit: ShadeHit, cx: f32, cy: f32, size: f32, ink: Rgba) {
        let m = &t.m;
        let stroke = (size * 0.075).max(m.dp(1.6));
        match hit {
            ShadeHit::Wlan => icons::glyph(out, Icon::Wlan, cx, cy, size, ink),
            ShadeHit::Sleep => icons::glyph(out, Icon::Power, cx, cy, size, ink),
            ShadeHit::Screenshot => {
                // A camera: its body, the bump over its viewfinder, its lens.
                let (w, h) = (size * 0.74, size * 0.54);
                let top = cy - h / 2.0 + size * 0.04;
                out.stroke_round_rect(cx - w / 2.0, top, w, h, size * 0.13, stroke, ink);
                out.fill_round_rect(cx - size * 0.12, top - size * 0.10, size * 0.24, size * 0.11, size * 0.045, ink);
                out.stroke_circle(cx, top + h / 2.0, size * 0.145, stroke, ink);
            }
            ShadeHit::Reboot => icons::glyph(out, Icon::Restore, cx, cy, size, ink),
            // The motor's mark is the slider's own: the tile and the capsule
            // below it stand for the same thing.
            ShadeHit::Vibrate | ShadeHit::Vibration => {
                // Three bars, the middle one longer: a motor tapping.
                for (i, tall) in [0.36f32, 0.68, 0.36].iter().enumerate() {
                    let x = cx + (i as f32 - 1.0) * size * 0.24;
                    out.fill_round_rect(x - size * 0.06, cy - size * tall / 2.0, size * 0.12, size * tall, size * 0.06, ink);
                }
            }
            ShadeHit::Transfer => icons::glyph(out, Icon::Usb, cx, cy, size, ink),
            ShadeHit::UsbDrive => icons::glyph(out, Icon::UsbDrive, cx, cy, size, ink),
            ShadeHit::Files => icons::glyph(out, Icon::Folder, cx, cy, size, ink),
            ShadeHit::Modules => icons::glyph(out, Icon::Layers, cx, cy, size, ink),
            ShadeHit::Partitions => icons::glyph(out, Icon::Mount, cx, cy, size, ink),
            ShadeHit::Terminal => icons::glyph(out, Icon::Terminal, cx, cy, size, ink),
            ShadeHit::Settings => icons::glyph(out, Icon::Gear, cx, cy, size, ink),
            ShadeHit::Brightness => {
                // A sun: its disc and eight rays.
                out.fill_circle(cx, cy, size * 0.19, ink);
                for k in 0..8 {
                    let a = k as f32 * std::f32::consts::TAU / 8.0;
                    let (sin, cos) = a.sin_cos();
                    let (r0, r1) = (size * 0.30, size * 0.45);
                    out.line(cx + cos * r0, cy + sin * r0, cx + cos * r1, cy + sin * r1, stroke, ink);
                }
            }
        }
    }

    /// Where the lock's knob is: the rect it is drawn in, which is the rect a
    /// finger has to land on.
    ///
    /// The rect the confirm control draws is the one a finger has to land on:
    /// its margins and height, not an inset of our own (2026-11-15, the knob
    /// answered nowhere while these differed). It is shorter than the page is
    /// wide, and centred: a control the whole width of the screen reads as a
    /// bar rather than something to slide.
    pub(super) fn lock_track(&self) -> Rect {
        let m = &self.theme().m;
        let wide = (m.w as f32 - m.margin * 2.0).min(m.dp(260.0));
        Rect::new((m.w as f32 - wide) / 2.0, m.h as f32 - m.dp(176.0), wide, m.dp(56.0))
    }

    /// The lock over the page: the phone's time, its battery, and the knob to
    /// slide along.
    fn compose_lock(&mut self, out: &mut Canvas, f: &Fonts, info: &DeviceInfo, clock: &str, drag: f32) {
        let t = self.theme();
        let m = &t.m;
        let (w, h) = (m.w as f32, m.h as f32);
        out.fill_rect(0.0, 0.0, w, h, Rgba::rgb(6, 6, 8).with_alpha(226));
        // The date under which the time stands, as a phone's lock screen shows.
        if let Ok(face) = f.face(400.0)
            && !self.date.is_empty()
        {
            let px = m.dp(20.0);
            let width = out.text_width(&face, &self.date, px);
            out.text(&face, &self.date, (w - width) / 2.0, h * 0.245, px, Rgba::rgb(158, 162, 172), None);
        }
        if let Ok(face) = f.face(600.0) {
            let px = m.dp(64.0);
            let baseline = h * 0.335;
            let width = out.text_width(&face, clock, px);
            out.text(&face, clock, (w - width) / 2.0, baseline, px, Rgba::rgb(236, 238, 242), None);
        }
        if let (Ok(face), Some(b)) = (f.face(400.0), info.battery.as_ref()) {
            // The level is always said, and while a cable is in, what it is
            // taking: 85% · 充电中 · 18W.
            let percent = format!("{}%", b.level);
            let note = match b.plugged {
                true => match b.watts() {
                    Some(watts) => format!("{percent} \u{00b7} {} \u{00b7} {watts:.0}W", self.state.lang.pick(["充电中", "充電中", "Charging"])),
                    None => format!("{percent} \u{00b7} {}", self.state.lang.pick(["充电中", "充電中", "Charging"])),
                },
                false => match b.watts() {
                    Some(watts) => format!("{percent} \u{00b7} {watts:.0}W"),
                    None => percent,
                },
            };
            let px = m.dp(18.0);
            let width = out.text_width(&face, &note, px);
            out.text(&face, &note, (w - width) / 2.0, h * 0.375, px, Rgba::rgb(150, 154, 164), None);
        }
        let track = self.lock_track();
        let (label, _) = crate::pages::confirm_label(Confirm::Unlock, &self.state);
        // Drawn in the rect `lock_track` gives, which is also where the finger
        // has to land: a control across the whole page is a bar, not a slide.
        self.lock_rect = Some(widgets::confirm_in(out, &t, f, track, label, false, drag));
    }

    /// The charging screen, `level` of the way over the page drawn on `out`.
    fn compose_charging(&mut self, out: &mut Canvas, f: &Fonts, info: &DeviceInfo, level: f32, now: u64) {
        let battery = info.battery.as_ref().expect("a cable went in on a phone whose battery is read");
        let (t, lang) = (self.theme(), self.state.lang);
        if !self.charge_art.as_ref().is_some_and(|a| a.shows(lang, battery)) {
            self.charge_art = Some(charge::Art::new(f, &t.m, lang, battery).expect("the screens' face takes the charging screen's weight"));
        }
        let since = self.charging.as_ref().map_or(now, |c| c.since);
        let dither = self.dither.get_or_insert_with(|| glow::Dither::new(out.w as usize, out.h as usize));
        let art = self.charge_art.as_ref().expect("drawn above");
        if level >= 1.0 {
            charge::draw(out, art, &t.m, battery.level, now.saturating_sub(since), dither);
        } else {
            let screen = self.charge_canvas.get_or_insert_with(|| Canvas::new(out.w, out.h));
            charge::draw(screen, art, &t.m, battery.level, now.saturating_sub(since), dither);
            out.lay(screen, 0, 0, level);
        }
    }

    /// The visible part of the page, pulled past an end if it is, its large
    /// title fading as it scrolls, the bar and its title over a scrolled
    /// page, the `<`, the page sliding in or out over or under the page
    /// left, the status bar, any message, and the back cue while a swipe
    /// from an edge is under way.
    fn compose_page(&mut self, out: &mut Canvas, f: &Fonts, info: &DeviceInfo, clock: &str, now: u64) {
        // A page opened is drawn from its left edge as it slides in.
        let opening = self.turn.as_ref().is_some_and(|t| !t.back);
        self.render(f, info, if opening { 0 } else { self.w });
        let screen = self.screen();
        let scroll = self.top().1;
        let offset = (scroll + self.overscroll(now)).round();
        let caret_on = match self.frame.as_ref().is_some_and(|fr| fr.caret.is_some()) {
            true => self.caret_up(screen, now),
            false => {
                self.blink = None;
                self.caret_shown = false;
                false
            }
        };
        if let Some(m) = &self.morph {
            if !m.at(now).1 {
                return self.compose_morph(out, f, info, clock, now, caret_on);
            }
            self.morph = None;
        }
        let t = self.theme();
        if let Some((which, track)) = self.knob.take() {
            self.paint_knob(&t, f, which, track);
        }
        if self.toast.as_ref().is_some_and(|m| now >= m.1) {
            self.toast = None;
        }
        let title = self.frame.as_ref().expect("rendered above").title;
        let key = (title, self.state.dark);
        if self.titles.as_ref().is_none_or(|(k, _, _)| *k != key) {
            self.titles = Some((key, Blurs::new(widgets::big_title(&t, f, title)), Blurs::new(widgets::small_title(&t, f, title))));
        }
        if let Some(fold) = self.frame.as_ref().and_then(|fr| fr.fold) {
            let key = (fold.hint, self.state.dark);
            if self.search_words.as_ref().is_none_or(|(k, _)| *k != key) {
                self.search_words = Some((key, widgets::search_words(&t, f, fold.bar, fold.hint)));
            }
        }
        if self.chip.as_ref().is_none_or(|c| c.0 != t.dark) {
            let (art, x, y) = widgets::back_chip(&t);
            self.chip = Some((t.dark, art, x, y));
        }
        let button = self.frame.as_ref().is_some_and(|fr| fr.button.is_some());
        if button && self.button_chip.as_ref().is_none_or(|c| c.0 != t.dark) {
            let (art, x, y) = widgets::button_chip(&t);
            self.button_chip = Some((t.dark, art, x, y));
        }
        let slide = self.slid(now);
        if self.frame.as_ref().is_some_and(|fr| fr.hero.is_none()) {
            // What of the page is on screen, drawn if it has not been yet.
            // While it slides in, that is its columns come in, and a
            // quarter of the screen more each frame, so it is drawn across
            // early in the slide; all of it once a row of it is held.
            let wide = match (slide, &self.turn) {
                (Some((at, _, _)), Some(turn)) if !turn.back && self.row_press.is_none() => {
                    (self.w - at.clamp(0, self.w as i32) as u32).max(self.cols + self.w / 4)
                }
                _ => self.w,
            };
            self.draw_cols(f, info, wide);
            self.draw_rows(f, info, offset, offset + self.h as f32);
        }
        // The row a finger holds, drawn once more as held down when it
        // starts darkening, and only that row: a finger that moves on
        // before then never waits for it.
        if let (Some(press), None) = (self.row_press.filter(|p| now >= p.rise), &self.row_art) {
            let page = &self.frame.as_ref().expect("rendered above").canvas;
            let (x, y, w, h) = row_part(press.rect, page);
            let pressed = t.pressed();
            let band = Band { rows: (y, y + h), cols: (x, x + w) };
            let cx = Ctx { t: &pressed, f, info, state: &self.state, term: self.term.as_ref(), band: Some(band) };
            let down = pages::render(screen, &cx).canvas;
            assert_eq!((down.w, down.h), (page.w, page.h), "a page held down is laid out as it was");
            self.row_art = Some((page.crop(x, y, w, h), down.crop(x, y, w, h)));
        }
        let view = View {
            scroll,
            offset,
            bar: self.bar.level(now),
            title: self.title.level(now),
            light: self.light(offset),
            caret: caret_on,
            row: self.row_press.map_or(0.0, |p| p.level(now)),
            pressing: self.pressing(),
        };
        match slide {
            Some((at, shift, dim)) => {
                // The page shown is drawn on its own canvas, once for as
                // long as it shows the same, and slid from there with the
                // page left.
                // A page opened shows nothing of itself until its edge comes
                // in from the right: the page left alone shows.
                let back = self.turn.as_ref().expect("sliding").back;
                if !back && at >= self.w as i32 {
                    let left = &self.turn.as_ref().expect("sliding").left;
                    out.slide(left, left, at, shift, dim, self.corners);
                } else {
                    let still = self.frame.as_ref().is_some_and(|fr| fr.hero.is_none());
                    let page = match self.layer.take() {
                        Some((shown, c)) if still && shown == view => c,
                        kept => {
                            let mut c = kept.map(|(_, c)| c).unwrap_or_else(|| Canvas::new(out.w, out.h));
                            self.paint_page(&mut c, &t, Some((f, info)), &view);
                            c
                        }
                    };
                    let turn = self.turn.as_ref().expect("sliding");
                    let (front, behind) = if back { (&turn.left, &page) } else { (&page, &turn.left) };
                    out.slide(front, behind, at, shift, dim, self.corners);
                    self.layer = Some((view, page));
                }
            }
            None => {
                self.layer = None;
                self.paint_page(out, &t, Some((f, info)), &view);
            }
        }
        self.shown = Some(view);

        widgets::status_bar(out, &t, f, &self.bar(clock, info), t.pal.status);
        if let Some((text, _)) = &self.toast {
            widgets::toast(out, &t, f, text);
        }
        match self.drag {
            Drag::Edge { start, from_left, armed, x, cy } => {
                let travel = if from_left { x - start } else { start - x };
                widgets::back_cue(out, &t, from_left, cy, widgets::back_cue_depth(&t, travel), armed);
            }
            _ => {
                if let Some((from_left, cy, s)) = self.cue {
                    widgets::back_cue(out, &t, from_left, cy, s.at(now).0.max(0.0), false);
                }
            }
        }
    }

    /// Draw the page shown as `view` has it: all of it but the status bar,
    /// any message and the back cue, which lie over every page. Rows of its
    /// long list come into view are drawn with `fresh`; a page drawn again
    /// as it last showed has them drawn already.
    pub(super) fn paint_page(&mut self, out: &mut Canvas, t: &Theme, fresh: Option<(&Fonts, &DeviceInfo)>, view: &View) {
        let m = t.m;
        let screen = self.screen();
        let (scroll, offset, pressing) = (view.scroll, view.offset, view.pressing);
        // The light goes over its control, in the part of the screen the
        // control is drawn in, under whatever is laid over that part.
        let shine = |out: &mut Canvas, part: Group| {
            if let Some((g, r, radius, tint)) = view.light
                && g == part
            {
                out.fill_round_rect(r.x, r.y, r.w, r.h, radius, tint);
            }
        };
        let frame = self.frame.as_ref().expect("a page is drawn from its frame");
        let (_, big, small) = self.titles.as_mut().expect("a page's titles are drawn with it");
        // The row a finger holds darkens as part of the page, under what is
        // laid over the page.
        let press_row = |out: &mut Canvas| {
            if let (Some(press), Some((was, is))) = (self.row_press, &self.row_art) {
                let (x, y, _, _) = row_part(press.rect, &frame.canvas);
                out.relay(was, is, x as i32, y as i32 - offset as i32, view.row);
            }
        };

        if let Some(art) = &frame.hero {
            // The field fills the screen and fades as the page scrolls; the
            // cards scroll over it. The version and the author's line fade
            // and shrink away first, then the wordmark.
            let glow = self.glow.as_ref().expect("the About page keeps its field");
            let fade = (1.0 - scroll / m.dp(ABOUT_FIELD_DP)).clamp(0.0, 1.0);
            let top = (widgets::bar_bottom(t) + m.dp(ABOUT_FIELD_DP)) / m.h as f32;
            let field = glow::sample(glow, out.w as usize, out.h as usize, FIELD_CELL, top, t.pal.bg, fade);
            let dither = self.dither.get_or_insert_with(|| glow::Dither::new(out.w as usize, out.h as usize));
            glow::paint(out, &field, fade, &glow::Layer { canvas: &frame.canvas, runs: &art.runs, offset: offset as isize }, dither);
            press_row(out);
            let range = m.dp(ABOUT_LOGO_DP) - widgets::bar_bottom(t);
            let v = (scroll / range).clamp(0.0, 1.0);
            art.version.draw_scaled(out, 0.0, 0.0, 1.0 - 0.1 * v, t.hero.version, 1.0 - v);
            art.author.draw_scaled(out, 0.0, 0.0, 1.0 - 0.1 * v, t.hero.version, 1.0 - v);
            let l = ((scroll - range) / m.dp(ABOUT_LOGO_FADE_DP)).clamp(0.0, 1.0);
            glow::ink_wordmark(out, &field, &glow.look().ink, &art.wordmark, 1.0 - 0.1 * l, 1.0 - l, fade, dither);
        } else {
            // The page rows on screen above a long list's rows, those rows,
            // and the page colour past the ends; every pixel is written
            // once.
            let h = out.h as f32;
            let span = frame.rows.as_ref().map(|r| ((r.edges[0] - offset).clamp(0.0, h), (r.bottom() - offset).clamp(0.0, h)));
            let (list_top, list_end) = span.unwrap_or((h, h));
            let first = (-offset).clamp(0.0, h) as usize;
            let last = (frame.canvas.h as f32 - offset).clamp(first as f32, list_top) as usize;
            if first < last {
                let from = offset as i32 + first as i32;
                out.put_rows(&frame.canvas, from, from + (last - first) as i32, first as i32);
            }
            out.fill_rect(0.0, 0.0, out.w as f32, first as f32, frame.bg);
            out.fill_rect(0.0, last as f32, out.w as f32, list_top - last as f32, frame.bg);
            let below = list_end.max(last as f32);
            out.fill_rect(0.0, below, out.w as f32, h - below, frame.bg);
            if let Some(rows) = &frame.rows {
                // A long list's rows on screen, each drawn when it first
                // shows. Those more than half a screen away are let go, and
                // their canvases drawn over again for the rows coming in, as
                // a list view recycles its rows.
                let near = rows.within(offset - h * NEAR_SCREENS, offset + h * (1.0 + NEAR_SCREENS));
                self.spare.extend(self.drawn.extract_if(.., |(i, _)| !near.contains(i)).map(|(_, c)| c));
                for i in rows.within(offset, offset + h) {
                    let k = match self.drawn.iter().position(|(j, _)| *j == i) {
                        Some(k) => k,
                        None => {
                            let (f, info) = fresh.expect("a page drawn again as it last showed has its rows in view drawn");
                            let mut c = self.spare.pop().unwrap_or_else(|| Canvas::new(out.w, 0));
                            c.resize((rows.edges[i + 1] - rows.edges[i]) as u32);
                            let cx = Ctx { t, f, info, state: &self.state, term: self.term.as_ref(), band: None };
                            pages::row(screen, &cx, i, &mut c);
                            self.drawn.push((i, c));
                            self.drawn.len() - 1
                        }
                    };
                    out.put(&self.drawn[k].1, (rows.edges[i] - offset) as i32);
                }
            } else {
                // Off a long list, its rows' canvases are not kept.
                self.spare.clear();
            }
            press_row(out);
            if let Some(fold) = &frame.fold {
                let (_, words) = self.search_words.as_ref().expect("a folding search bar's words are made with its page");
                fold.draw(out, t, words, offset);
            }
            if frame.header {
                let level = (1.0 - frame.title_lift(scroll) / m.dp(TITLE_FADE_DP)).clamp(0.0, 1.0);
                draw_faded(out, big, 0.0, -frame.title_lift(offset), t.pal.text, level, (1.0 - level) * m.dp(TITLE_BLUR_DP));
            }
        }
        // The search page's round button and the home page's search bar
        // darken at once under a finger.
        if let Some(a) = pressing.filter(|a| matches!(a, Action::CloseSearch | Action::Open(Screen::Search))) {
            let r = frame.hits.iter().find(|h| h.action == a).expect("a button held down is on its page").rect;
            match frame.shown(r, offset) {
                Some(r) if a == Action::CloseSearch => widgets::cancel_held(out, t, r),
                Some(r) => widgets::search_bar_held(out, t, r),
                None => {}
            }
        }
        if let Some(caret) = frame.caret.as_ref().filter(|_| view.caret) {
            out.lay(&caret.art, caret.x, caret.y - offset as i32, 1.0);
        }
        let bar = view.bar;
        let strip = frame.pinned.as_ref().map(|p| (p, pinned_top(t, p, offset)));
        shine(out, Group::Page);
        if bar > 0.0 {
            match strip {
                // The strip under the bar is veiled with it, and the list
                // thins out below the strip.
                Some((p, top)) => widgets::strip_mask(out, t, frame.bg, bar, top.round(), top.round() + p.canvas.h as f32),
                None => {
                    let stops = if frame.hero.is_some() { &widgets::ABOUT_BAR_MASK[..] } else { &widgets::BAR_MASK[..] };
                    widgets::bar_mask(out, t, stops, frame.bg, bar);
                }
            }
        }
        if let Some((p, top)) = strip {
            out.put(&p.canvas, top.round() as i32);
            shine(out, Group::Strip);
        }
        if let Some(d) = &frame.dock {
            out.put(&d.canvas, (out.h - d.canvas.h) as i32);
            shine(out, Group::Dock);
        }
        if frame.back {
            let (_, chip, x, y) = self.chip.as_ref().expect("a page's `<` is drawn with it");
            out.lay(chip, *x, *y, bar);
            if pressing == Some(Action::Back) {
                widgets::back_held(out, t);
            } else {
                widgets::back_arrow(out, t, t.pal.text);
            }
        }
        if let Some((glyph, action)) = frame.button {
            let (_, chip, x, y) = self.button_chip.as_ref().expect("a page's round button is drawn with its chip");
            out.lay(chip, *x, *y, bar);
            if pressing == Some(action) {
                widgets::button_held(out, t, glyph);
            } else {
                widgets::button_glyph(out, t, glyph, t.pal.text);
            }
        }
        if frame.back || frame.button.is_some() {
            shine(out, Group::Fixed);
        }
        let level = view.title;
        let blur = if frame.hero.is_some() { 0.0 } else { (1.0 - level) * m.dp(TITLE_BLUR_DP) };
        draw_faded(out, small, 0.0, 0.0, t.pal.text, level, blur);
    }

    /// Where a page sliding in over the page left, or the page left
    /// sliding off, stands at `now`: how many pixels from the left edge the
    /// page in front stands, how many left of where it rests the page
    /// behind, and how dark that is; nothing once the slide is over. See
    /// [`Turn`].
    fn slid(&mut self, now: u64) -> Option<(i32, i32, f32)> {
        let turn = self.turn.as_mut()?;
        let spring = *turn.spring.get_or_insert_with(|| {
            if turn.back {
                Spring::new(now, -1000.0, 0.0, BACK_PERIOD)
            } else {
                Spring::new(now, 1000.0, -OPEN_SPEED * 1000.0, OPEN_PERIOD)
            }
        });
        let (x, done) = spring.at(now);
        if done {
            self.backdrop = self.turn.take().map(|t| t.left);
            return None;
        }
        // Where the page in front stands, in screen widths from the left
        // edge.
        let along = if turn.back { 1.0 + x / 1000.0 } else { x / 1000.0 };
        let w = self.w as f32;
        Some(((along * w).round() as i32, (BEHIND_SHIFT * w * (1.0 - along)).round() as i32, BEHIND_DIM * (1.0 - along)))
    }

    /// Whether the cursor of the text field on `screen` shows at `now`: it
    /// comes up whole on each page it shows on and at each edit, then
    /// blinks.
    fn caret_up(&mut self, screen: Screen, now: u64) -> bool {
        let key = (screen, self.edits);
        if self.blink.is_none_or(|(s, e, _)| (s, e) != key) {
            self.blink = Some((screen, self.edits, now));
        }
        self.caret_shown = self.blink_at(now).is_some_and(|(on, _)| on);
        self.caret_shown
    }

    /// Draw the search bar on its way between the home page and the search
    /// page; see [`Morph`].
    fn compose_morph(&mut self, out: &mut Canvas, f: &Fonts, info: &DeviceInfo, clock: &str, now: u64, caret_on: bool) {
        let t = self.theme();
        let m = t.m;
        let mut morph = self.morph.take().expect("composing it");
        let (q, _) = morph.at(now);
        let (field, _) = widgets::search_layout(&t);
        let home = Rect::new(m.margin, m.content_top - morph.scroll, m.w as f32 - m.margin * 2.0, m.search_h);
        let along = |a: f32, b: f32| a + (b - a) * q;
        let bar = Rect::new(m.margin, along(home.y, field.y), along(home.w, field.w), m.search_h);
        let rise = bar.y - home.y;
        if !morph.into {
            // The home page's rows coming down with its bar.
            self.draw_rows(f, info, morph.scroll - rise, morph.scroll - rise + m.h as f32);
        }
        let frame = self.frame.as_ref().expect("rendered above");
        out.fill_rect(0.0, 0.0, out.w as f32, out.h as f32, frame.bg);
        if morph.into {
            // The search page's keyboard and what it shows under its
            // field, from its frame; that comes in once the bar is nearly
            // there.
            let (_, keys_top) = *morph.keyboard.get_or_insert_with(|| keyboard_strip(&t, frame));
            let (body, top) = morph.body.get_or_insert_with(|| {
                let top = (field.y + field.h).ceil() as i32;
                let mut body = Canvas::new(frame.canvas.w, (keys_top - top).max(0) as u32);
                body.put_rows(&frame.canvas, top, keys_top, 0);
                (body, top)
            });
            let (delay, span) = MORPH_BODY_MS;
            out.lay(body, 0, *top, (now.saturating_sub(morph.start + delay) as f32 / span as f32).min(1.0));
        } else {
            // The home page under its bar comes down with it.
            let below = m.content_top + m.search_h;
            out.put_rows(&frame.canvas, below.floor() as i32, frame.canvas.h as i32, (below - morph.scroll + rise).floor() as i32);
        }
        let title = morph.title.get_or_insert_with(|| Blurs::new(widgets::big_title(&t, f, pages::HOME_TITLE)));
        let level = (1.0 - q / MORPH_TITLE_GONE).clamp(0.0, 1.0);
        draw_faded(out, title, 0.0, rise - morph.scroll, t.pal.text, level, (1.0 - level) * m.dp(TITLE_BLUR_DP));
        if let Some((keys, top)) = &morph.keyboard {
            out.put(keys, (*top as f32 + (1.0 - q) * keys.h as f32).round() as i32);
        }
        widgets::search_capsule(out, &t, f, bar, self.state.lang.pick(pages::SEARCH_HINT));
        // The cursor shows from the start, going up with the field.
        if let Some(caret) = frame.caret.as_ref().filter(|_| morph.into && caret_on) {
            out.lay(&caret.art, caret.x, caret.y + (bar.y - field.y).round() as i32, 1.0);
        }
        let (disc, cross) = morph.cancel.get_or_insert_with(|| {
            let (disc, cross) = widgets::cancel_art(&t);
            (Blurs::new(disc), Blurs::new(cross))
        });
        let shown = ((q - MORPH_CANCEL.0) / (MORPH_CANCEL.1 - MORPH_CANCEL.0)).clamp(0.0, 1.0);
        let blur = (1.0 - shown) * m.dp(MORPH_CANCEL_BLUR_DP);
        draw_faded(out, disc, 0.0, 0.0, t.pal.search, shown, blur);
        draw_faded(out, cross, 0.0, 0.0, t.pal.text, shown, blur);
        self.morph = Some(morph);
        widgets::status_bar(out, &t, f, &self.bar(clock, info), t.pal.status);
        if let Some((text, _)) = &self.toast {
            widgets::toast(out, &t, f, text);
        }
    }

    /// Put the page back where it rests, for a page just opened or returned
    /// to; its first frame shows the bar as its scroll has it.
    pub(super) fn settle_page(&mut self) {
        self.height = 0.0;
        self.fling = None;
        self.glide = None;
        self.pull = 0.0;
        self.bounce = None;
        self.seek = None;
        self.adjusting = false;
        self.titles = None;
        self.glow = None;
        self.row_press = None;
        self.row_art = None;
        let hero = self.screen() == Screen::About;
        self.title = title_fade(hero);
        self.bar = bar_fade(hero);
        self.snap = true;
    }

    /// Show the user what the runtime has to say about work it was asked
    /// to do.
    pub fn report(&mut self, text: &str, now: u64) {
        self.say(text, now);
    }

    /// The screenshot [`Effect::Screenshot`] asked for went where `saved`
    /// says, or did not, as the system says why.
    pub fn screenshot_saved(&mut self, saved: Result<Shot, String>, now: u64) {
        let l = self.state.lang;
        let text = match saved {
            Ok(Shot::Album) => l.pick(fmt::SCREENSHOT_SAVED).to_string(),
            Ok(Shot::Held) => l.pick(fmt::SCREENSHOT_HELD).to_string(),
            Err(why) => fmt::screenshot_failed(l, &why),
        };
        self.say(&text, now);
    }

    /// The screenshots held while the owner's storage was closed did not go
    /// into the gallery's once it opened, as the system says why.
    pub fn screenshots_not_kept(&mut self, why: &str, now: u64) {
        self.say(&fmt::screenshots_not_kept(self.state.lang, why), now);
    }

    /// File transfer could not be turned on or off as the switch says: it
    /// shows how it stands, and why.
    pub fn file_transfer_failed(&mut self, on: bool, why: &str, now: u64) {
        self.state.file_transfer = on;
        self.refresh();
        self.say(why, now);
    }

    pub(super) fn say(&mut self, text: &str, now: u64) {
        self.toast = Some((text.to_string(), now + TOAST_MS));
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::*;
    use skyward_info::Charge;

    #[test]
    fn a_page_drawn_a_band_at_a_time_is_the_page_drawn_whole() {
        let (f, i) = (fonts(), info());
        let state = State::default();
        // The About page lies over its field and is drawn whole.
        let screens = [
            Screen::Decrypt,
            Screen::Home,
            Screen::Settings,
            Screen::Reboot,
            Screen::Wipe,
            Screen::Mount,
            Screen::Install,
            Screen::Choose(Place::Storage),
            Screen::Specs,
            Screen::ScreenOff,
            Screen::Search,
            Screen::Backup,
            Screen::Restore,
            Screen::Advanced,
            Screen::Files,
            Screen::Terminal,
            Screen::Language,
            Screen::TimeZone,
            Screen::TimeZones(Region::Asia),
            Screen::Logs,
            Screen::Update,
            Screen::SkywardUpdate,
            Screen::License,
            Screen::FontLicense,
            Screen::Charge,
            Screen::Slot,
            Screen::Fastboot,
            Screen::Erase,
            Screen::CantStart,
            Screen::Unhandled,
        ];
        for t in [Theme::light(W, H, PPD), Theme::dark(W, H, PPD).pressed()] {
            for screen in screens {
                let cx = |band| Ctx { t: &t, f: &f, info: &i, state: &state, term: None, band };
                let whole = pages::render(screen, &cx(None)).canvas;
                let row = whole.w as usize * 4;
                let differs = |c: &Canvas| c.data.chunks(row).zip(whole.data.chunks(row)).position(|(a, b)| a != b);
                // Cuts through cards, text and icons, and a band one row
                // tall, the bands joined one under another.
                let mut cuts: Vec<u32> = [0, 1, 377, 1201, 3000, 3001, 4444, u32::MAX].map(|k| k.min(whole.h)).to_vec();
                cuts.dedup();
                let mut joined: Option<Canvas> = None;
                for rows in cuts.windows(2) {
                    let part = pages::render(screen, &cx(Some(Band { rows: (rows[0], rows[1]), cols: (0, W) }))).canvas;
                    assert_eq!((part.h, part.held()), (whole.h, rows[0]..rows[1]), "{screen:?}: a band is laid out as the whole");
                    match &mut joined {
                        Some(c) => c.join(&part),
                        None => joined = Some(part),
                    }
                }
                let joined = joined.expect("a page has rows");
                assert!(joined.whole(), "{screen:?}: the bands make the whole");
                assert_eq!(differs(&joined), None, "{screen:?}: the first row drawn otherwise in bands");
                // The page drawn across a part at a time, as one sliding
                // in, into a canvas holding all of it: cuts through cards
                // and text, and a part one column wide.
                let mut across = Canvas::unfilled(W, whole.h, 0..whole.h);
                for cols in [0, 1, 333, 720, W - 1, W].windows(2) {
                    let part = pages::render(screen, &cx(Some(Band { rows: (0, whole.h), cols: (cols[0], cols[1]) }))).canvas;
                    assert_eq!(part.cols(), cols[0]..cols[1], "{screen:?}: a part across holds its columns");
                    across.replace(&part);
                }
                assert_eq!(differs(&across), None, "{screen:?}: the first row drawn otherwise across");
            }
        }
    }

    #[test]
    fn a_page_at_rest_draws_the_rows_below_the_screen_a_band_at_a_time() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::Settings, W, H, PPD);
        app.compose(&mut Canvas::new(W, H), &f, &i, "12:00", 0);
        assert_eq!(app.frame.as_ref().unwrap().canvas.held(), 0..H, "the page shown has the rows on screen drawn");
        let mut bands = 1;
        while app.draw_ahead(&f, &i) {
            bands += 1;
        }
        let t = app.theme();
        let whole = pages::render(Screen::Settings, &Ctx { t: &t, f: &f, info: &i, state: &app.state, term: None, band: None }).canvas;
        let page = &app.frame.as_ref().unwrap().canvas;
        assert!(whole.h > H + H / 4, "a page well over a screen long, drawn in {bands} more bands");
        assert!(page.whole() && page.data == whole.data, "the page drawn a band at a time is the page drawn whole");
    }

    #[test]
    fn a_page_typed_on_draws_again_only_what_changed_and_shows_what_it_would_drawn_anew() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::Search, W, H, PPD);
        let mut out = Canvas::new(W, H);
        app.compose(&mut out, &f, &i, "12:00", 0);
        let anew = |app: &App| {
            let t = app.theme();
            pages::render(Screen::Search, &Ctx { t: &t, f: &f, info: &i, state: &app.state, term: None, band: None })
        };
        for cap in [Cap::Char('b'), Cap::Char('f'), Cap::Space, Cap::Backspace, Cap::Shift, Cap::Char('q')] {
            let (x, y) = spot(&mut app, &f, &i, Action::Type(cap));
            app.touch(Touch::Down(x, y), &f, 0);
            assert!(app.frame.is_some() && !app.stale.is_empty() || cap == Cap::Shift, "{cap:?}: the key held is drawn again on its own");
            app.compose(&mut out, &f, &i, "12:00", 0);
            assert!(app.frame.as_ref().unwrap().canvas.data == anew(&app).canvas.data, "{cap:?} held down");
            app.touch(Touch::Up, &f, 0);
            app.compose(&mut out, &f, &i, "12:00", 0);
            let (drawn, fresh) = (app.frame.as_ref().unwrap(), anew(&app));
            assert!(drawn.canvas.data == fresh.canvas.data, "{cap:?} typed");
            assert_eq!(drawn.hits.iter().map(|h| h.action).collect::<Vec<_>>(), fresh.hits.iter().map(|h| h.action).collect::<Vec<_>>(), "{cap:?} typed");
        }
        assert_eq!(app.state.search, "bfQ");
    }

    #[test]
    fn the_home_card_tells_of_a_battery_too_low_to_install_on() {
        let f = fonts();
        // Where the first row under the card starts: the card is that much
        // taller with a notice in it.
        let under = |i: &DeviceInfo| {
            let mut app = App::new(Screen::Home, W, H, PPD);
            app.frame(&f, i).hits.iter().find(|h| h.action == Action::Open(Screen::Install)).expect("an install row").rect.y
        };
        let plain = under(&info());
        assert!(under(&DeviceInfo { battery: Some(battery_at(19, Charge::Discharging)), ..info() }) > plain, "short of 20% unplugged");
        assert_eq!(under(&DeviceInfo { battery: Some(battery_at(20, Charge::Discharging)), ..info() }), plain);
        assert!(under(&DeviceInfo { battery: Some(battery_at(14, Charge::Charging)), ..info() }) > plain, "short of 15% charging");
        assert_eq!(under(&DeviceInfo { battery: Some(battery_at(16, Charge::Charging)), ..info() }), plain);
        let (title, note) = fmt::battery_low(Lang::Hans, 14, true);
        assert_eq!((title.as_str(), note.as_str()), ("电量仅剩 14%", "充电至 15% 以上才能安装"));
    }

    #[test]
    fn the_home_card_tells_of_a_restart_into_the_other_slot_and_opens_the_slot_page() {
        let f = fonts();
        let i = DeviceInfo { slot: Some(Slot::B), ..info() };
        let mut app = App::new(Screen::Home, W, H, PPD);
        let slots = |active| BootSlots { active, bootable: [true, true], successful: [true, true] };
        app.set_boot_slots(Ok(slots(Slot::B)));
        assert!(app.frame(&f, &i).hits.iter().all(|h| h.action != Action::Open(Screen::Slot)), "the slot it runs from starts next");
        app.set_boot_slots(Ok(slots(Slot::A)));
        assert_eq!(tap(&mut app, &f, &i, Action::Open(Screen::Slot)), vec![Effect::ReadSlots]);
        assert_eq!(app.screen(), Screen::Slot);
        assert_eq!(fmt::next_boot(Lang::Hans, Slot::A, Slot::B), ("重启后从 A 槽位启动".into(), "当前运行的是 B 槽位".into()));
    }

    #[test]
    fn the_status_bar_carries_the_slot_the_data_partitions_lock_and_the_temperature() {
        let i = DeviceInfo { slot: Some(Slot::B), battery: Some(battery_at(64, Charge::Charging)), ..info() };
        let mut app = App::new(Screen::Home, W, H, PPD);
        let bar = app.bar("12:00", &i);
        assert_eq!((bar.slot, bar.temp, bar.battery), (Some("B"), Some(352), Some((64, Some(Bolt::Charging)))), "the temperature to the tenth");
        let quick = DeviceInfo { battery: Some(skyward_info::Battery { quick: true, ..battery_at(64, Charge::Charging) }), ..info() };
        assert_eq!(app.bar("12:00", &quick).battery, Some((64, Some(Bolt::Quick))), "a quick charger draws the stock bar's quick bolt");
        let out = DeviceInfo { battery: Some(battery_at(64, Charge::Discharging)), ..info() };
        assert_eq!(app.bar("12:00", &out).battery, Some((64, None)), "no bolt with the cable out");
        let lock = |app: &mut App, data: Data| {
            app.state.data = data;
            app.bar("12:00", &i).data
        };
        assert_eq!(lock(&mut app, Data::Locked { user: 0, keys: Keys::Digits, checking: false }), Some(false), "shut while its files are out of reach");
        assert_eq!(lock(&mut app, Data::Open), Some(true));
        assert_eq!(lock(&mut app, Data::Erased), None, "nothing to open once wiped");
        let none = app.bar("12:00", &info());
        assert_eq!((none.slot, none.temp), (None, None), "a phone without slots or a fuel gauge");
    }

    #[test]
    fn the_cursor_blinks_and_comes_up_whole_at_each_edit() {
        let (f, i) = (fonts(), info());
        let mut app = opened(Screen::Search, &f, &i);
        let mut screen = Canvas::new(W, H);
        let mut shows = |app: &mut App, now: u64| {
            app.compose(&mut screen, &f, &i, "12:00", now);
            app.caret_shown
        };
        assert!(shows(&mut app, 1000), "it comes up whole on the page");
        assert_eq!(app.deadline(1200), Some(1500), "the loop wakes when it turns");
        assert!(!shows(&mut app, 1500), "half a second on it hides");
        assert!(shows(&mut app, 2000));
        assert!(!shows(&mut app, 2600));
        tap(&mut app, &f, &i, Action::Type(Cap::Char('b')));
        assert!(shows(&mut app, 2700), "an edit brings it back whole at once");
        assert!(!shows(&mut app, 3200));
        tap(&mut app, &f, &i, Action::CloseSearch);
        assert_eq!(app.screen(), Screen::Home, "the round button leaves the search");
        assert!(!shows(&mut app, 3300), "no cursor off the page");
        assert!(app.deadline(3300).is_none_or(|d| d > 10_000), "nor a wake for it, only for the screen going off");
    }

    #[test]
    fn the_search_bar_moves_up_into_the_search_page_and_back_down() {
        let (f, i) = (fonts(), info());
        let t = Theme::light(W, H, PPD);
        let m = t.m;
        let mut app = App::new(Screen::Home, W, H, PPD);
        let mut screen = Canvas::new(W, H);
        // The bar's top along a column through it, clear of the title and
        // the placeholder: the first row in the field's grey.
        let x = m.dp(300.0) as u32;
        let top = |screen: &Canvas| {
            (m.dp(40.0) as u32..H).find(|&y| screen.data[((y * W + x) * 4) as usize] == t.pal.search.r).map(|y| y as f32)
        };
        let grey = |screen: &Canvas, y: f32| screen.data[((y as u32 * W + W / 2) * 4) as usize];
        let (home, field) = (m.content_top, widgets::search_layout(&t).0.y);
        // The device card under the home page's bar.
        let card = m.content_top + m.search_h + m.after_search + m.dp(20.0);
        app.compose(&mut screen, &f, &i, "12:00", 0);
        assert_eq!(grey(&screen, card), t.pal.card.r);

        tap_at(&mut app, &f, &i, Action::Open(Screen::Search), 1000);
        assert_eq!(app.screen(), Screen::Search);
        assert_eq!(app.deadline(1000), Some(0), "it moves frame by frame");
        app.compose(&mut screen, &f, &i, "12:00", 1067);
        let along = crate::motion::settle(67, MORPH_PERIOD);
        let expected = home + (field - home) * along;
        let at = top(&screen).expect("the bar is on screen");
        assert!((at - expected).abs() <= 2.0, "up {along} of the way after 67 ms: at {at}, not {expected}");
        assert_eq!(grey(&screen, card), t.pal.bg.r, "the home page's cards go at once");
        app.compose(&mut screen, &f, &i, "12:00", 2500);
        assert!(app.morph.is_none(), "settled, the search page shows as itself");
        assert_eq!(top(&screen), Some(field.ceil()));

        tap_at(&mut app, &f, &i, Action::CloseSearch, 3000);
        assert_eq!(app.screen(), Screen::Home);
        app.compose(&mut screen, &f, &i, "12:00", 3067);
        let expected = field + (home - field) * along;
        let at = top(&screen).expect("the bar is on screen");
        assert!((at - expected).abs() <= 2.0, "down {along} of the way after 67 ms: at {at}, not {expected}");
        assert_eq!(grey(&screen, card + (at - home)), t.pal.card.r, "the cards come down under the bar");
        app.compose(&mut screen, &f, &i, "12:00", 4500);
        assert!(app.morph.is_none());
        assert_eq!(top(&screen), Some(home.ceil()));
    }

    #[test]
    fn the_control_centre_lays_out_twelve_tiles_and_every_one_answers_a_touch() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::Home, W, H, PPD);
        let mut c = Canvas::new(W, H);
        app.shade = 1.0;
        app.compose(&mut c, &f, &i, "12:00", 1000);
        // The twelve tiles and the two capsules, and nothing else.
        assert_eq!(app.shade_hits.len(), 14, "twelve tiles and two capsules");
        let tiles: Vec<ShadeHit> = app.shade_hits.iter().map(|(_, h)| *h).filter(|h| !matches!(h, ShadeHit::Brightness | ShadeHit::Vibration)).collect();
        assert_eq!(tiles, ShadeHit::TILES.to_vec(), "the tiles in the order the sheet lays them out");
        // Every one of them sits inside the sheet, and no two overlap: a tile
        // a finger cannot reach, or one that takes another's touch, is a tile
        // that does nothing.
        let sheet = app.theme().m.dp(SHADE_HEIGHT_DP);
        for (k, (rect, hit)) in app.shade_hits.iter().enumerate() {
            assert!(rect.y >= 0.0 && rect.y + rect.h <= sheet, "{hit:?} runs past the sheet: {rect:?}");
            assert!(rect.x >= 0.0 && rect.x + rect.w <= W as f32, "{hit:?} runs off the side: {rect:?}");
            for (o, oh) in app.shade_hits.iter().skip(k + 1) {
                let apart = rect.x + rect.w <= o.x || o.x + o.w <= rect.x || rect.y + rect.h <= o.y || o.y + o.h <= rect.y;
                assert!(apart, "{hit:?} and {oh:?} overlap");
            }
        }
        // The rows are three, and the four of each sit on the same line.
        let ys: Vec<f32> = app.shade_hits.iter().filter(|(_, h)| ShadeHit::TILES.contains(h)).map(|(r, _)| r.y).collect();
        assert_eq!(ys.chunks(4).count(), 3, "three rows of four");
        for row in ys.chunks(4) {
            assert!(row.windows(2).all(|w| (w[0] - w[1]).abs() < 0.01), "a row is level: {row:?}");
        }
        assert!(ys[0] < ys[4] && ys[4] < ys[8], "and the rows run down the sheet");
    }

    #[test]
    fn the_control_centre_lights_the_tiles_that_turn_something_on() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::Home, W, H, PPD);
        let mut c = Canvas::new(W, H);
        app.shade = 1.0;
        // Nothing on: the WLAN is off, the motor still, file transfer off and
        // no drive plugged in.
        app.state.wlan = None;
        app.state.vibrate = false;
        app.state.file_transfer = false;
        app.state.usb_drive = None;
        app.compose(&mut c, &f, &i, "12:00", 1000);
        let lit = |app: &App| -> Vec<ShadeHit> { ShadeHit::TILES.into_iter().filter(|h| h.lit(&app.state)).collect() };
        assert!(lit(&app).is_empty(), "nothing is lit with everything off");
        // Turned on, each shows lit; the ones that open a page never do.
        app.state.wlan = Some(connected("Redmi-AX6000"));
        app.state.vibrate = true;
        app.state.file_transfer = true;
        app.state.usb_drive = Some(UsbDrive::Shown);
        app.compose(&mut c, &f, &i, "12:00", 1000);
        assert_eq!(lit(&app), vec![ShadeHit::Wlan, ShadeHit::Vibrate, ShadeHit::Transfer, ShadeHit::UsbDrive]);
        assert!(!ShadeHit::Files.lit(&app.state) && !ShadeHit::Settings.lit(&app.state), "a page's tile is never lit");
    }

    #[test]
    fn a_name_too_long_for_its_tile_is_set_smaller_before_it_is_rolled() {
        let (f, i) = (fonts(), info());
        let t = Theme::light(W, H, PPD);
        let mut app = App::new(Screen::Home, W, H, PPD);
        let mut c = Canvas::new(W, H);
        // A name that fits at the tile's own size is not rolled. The tiles
        // are a quarter of the sheet wide, so the name has to be short.
        app.state.wlan = Some(connected("Mi"));
        app.shade = 1.0;
        app.compose(&mut c, &f, &i, "12:00", 1000);
        assert!(app.shade_roll.is_none(), "a name that fits is still");

        // One far too long for any size that fits: it rolls.
        let long = "一个很长很长的无线网络名称用来看看跑马灯";
        app.state.wlan = Some(connected(long));
        app.compose(&mut c, &f, &i, "12:00", 2000);
        let roll = app.shade_roll.expect("a name too long to set whole rolls");
        assert!(roll.travel > roll.room, "wider than the tile at its least size");
        assert_eq!(roll.since, 2000, "it sets off the first time it is drawn");
        // The least size is the floor, not smaller.
        let least = t.m.dp(SHADE_NAME_LEAST_DP);
        let room = roll.room;
        let at_least = c.text_width(&f.body, long, least);
        assert!(at_least > room, "the floor does not fit either, so it rolls");
    }

    #[test]
    fn a_rolling_name_keeps_its_place_and_asks_for_frames_only_while_it_moves() {
        let (f, i) = (fonts(), info());
        let mut app = App::new(Screen::Home, W, H, PPD);
        let mut c = Canvas::new(W, H);
        app.state.wlan = Some(connected("一个很长很长的无线网络名称用来看看跑马灯"));
        app.shade = 1.0;
        app.compose(&mut c, &f, &i, "12:00", 1000);
        let first = app.shade_roll.expect("rolling");
        // A frame later the name has not set off again.
        app.compose(&mut c, &f, &i, "12:00", 1040);
        assert_eq!(app.shade_roll.expect("still rolling").since, first.since, "it keeps its place");

        // Resting at its start, it asks for nothing: the sheet is drawn every
        // turn while it is out, and a frame a turn would copy the panel for a
        // name that is standing still.
        app.tick(first.since + 100);
        let _ = app.take_dirty();
        app.tick(first.since + 200);
        assert!(!app.take_dirty(), "at rest it does not have the panel copied");

        // On the move it does, every turn it moves.
        let moving = first.since + SHADE_ROLL_REST_MS + 100;
        app.tick(moving);
        assert!(app.take_dirty(), "a moving name has the panel copied");
        app.tick(moving + 100);
        assert!(app.take_dirty(), "and again while it is still moving");
    }

    #[test]
    fn a_rolled_name_never_leaves_its_tile() {
        // The offset stays between the name's start and its end, and comes
        // back: nothing of the round trip runs past the tile.
        let (travel, room) = (900.0, 400.0);
        let over = travel - room;
        for ms in (0..40_000).step_by(7) {
            let at = roll_offset(travel, room, 66.0, ms);
            assert!((0.0..=over).contains(&at), "{ms} ms: {at}");
        }
        // It rests at the start, then at the end, and comes back.
        assert_eq!(roll_offset(travel, room, 66.0, 0), 0.0);
        let roll = (over / 66.0 * 1000.0) as u64;
        assert!(roll_offset(travel, room, 66.0, SHADE_ROLL_REST_MS + roll) >= over - 0.01, "the end shows");
        assert!(roll_offset(travel, room, 66.0, SHADE_ROLL_REST_MS + roll + SHADE_ROLL_REST_MS + roll) <= 0.01, "and it comes back");
        // A name that fits does not move at all.
        assert_eq!(roll_offset(room, room, 66.0, 5000), 0.0);
        assert_eq!(roll_offset(room - 1.0, room, 66.0, 5000), 0.0);
    }
}
