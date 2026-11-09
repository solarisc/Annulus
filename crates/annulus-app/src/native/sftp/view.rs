//! SFTP pane, dialog, and transfer drawing.

use super::*;

impl SftpView {
    /// The card asking what to do with the first name taken (F-SFTP-005): Skip, Replace (for a
    /// folder, Merge) or Rename under the name in its field, which starts free.
    #[allow(clippy::too_many_lines)] // One card, top to bottom.
    pub(super) fn clash_card(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        if self
            .clashes
            .front()
            .and_then(|clash| clash.waiting_on.as_ref())
            .is_some_and(|target| self.copying.contains(target))
        {
            return;
        }
        let waiting = self.clashes.len().saturating_sub(1);
        let Some(clash) = self.clashes.front_mut() else {
            return;
        };
        let t = ui.theme;
        let height = if waiting > 0 { 330.0 } else { 296.0 };
        ui.zone(Id::new("sftp.clash.backdrop"), rect, Role::Backdrop, "");
        let card = rect.centered(rect.w.min(500.0), height);
        let card_id = Id::new(("sftp.clash", clash.name.as_str()));
        let first = !ui.state.focus().is_some_and(|focus| {
            [
                Id::new("sftp.clash.name"),
                Id::new("sftp.clash.skip"),
                Id::new("sftp.clash.replace"),
                Id::new("sftp.clash.rename"),
                Id::new("sftp.clash.all"),
            ]
            .contains(&focus)
        });
        pages::card_in(ui, card_id, rect, card);
        ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
        let mut column = card.inset(space::XL, space::XL);
        let mut buttons = Rect::new(column.x, column.b() - CONTROL, column.w, CONTROL);
        let what = if clash.folder { "A folder" } else { "A file" };
        ui.strict_text(
            column.take_top(24.0),
            &format!("{what} by this name is already there"),
            HEADING,
            t.text,
            Align::Start,
        );
        column.take_top(space::S);
        let fitted =
            annulus_ui::gfx::fit_middle(&mut ui.gfx, &clash.name, Font::mono(13.0), column.w);
        ui.gfx.text(
            column.take_top(20.0),
            &fitted,
            Font::mono(13.0),
            t.text,
            Align::Start,
        );
        let into = format!("in {}", clash.place);
        let fitted = annulus_ui::gfx::fit_path(&mut ui.gfx, &into, SMALL, column.w);
        ui.gfx
            .text(column.take_top(18.0), &fitted, SMALL, t.text2, Align::Start);
        column.take_top(space::S);
        for (label, value) in [("There now", &clash.there), ("Coming", &clash.coming)] {
            let mut line = column.take_top(20.0);
            ui.gfx
                .text(line.take_left(90.0), label, SMALL, t.text3, Align::Start);
            ui.gfx.text(line, value, SMALL, t.text2, Align::Start);
        }
        column.take_top(space::M);
        let field_id = Id::new("sftp.clash.name");
        if first {
            ui.state.set_focus(Some(field_id), false);
        }
        let typed = clash.rename.trim().to_owned();
        let taken = clash.taken.contains(&typed);
        let bad = !valid_name(&typed) || taken;
        let field = ui.text_field(
            field_id,
            column.take_top(CONTROL),
            &mut clash.rename,
            FieldStyle {
                placeholder: "New name",
                mono: true,
                error: bad,
                ..FieldStyle::default()
            },
        );
        ui.set_name(field_id, "Rename to");
        column.take_top(space::XS);
        let hint = if taken {
            "That name is taken too."
        } else if bad {
            "A name, without / or a folder of its own."
        } else {
            "Rename keeps both, under this name."
        };
        ui.gfx.text(
            column.take_top(18.0),
            hint,
            SMALL,
            if bad { t.error } else { t.text3 },
            Align::Start,
        );
        if waiting > 0 {
            column.take_top(space::S);
            let label = format!(
                "Do the same for the {waiting} other{} waiting",
                if waiting == 1 { "" } else { "s" }
            );
            ui.checkbox(
                Id::new("sftp.clash.all"),
                column.take_top(24.0),
                &mut self.clash_all,
                &label,
            );
        }
        let replace_label = if clash.folder { "Merge" } else { "Replace" };
        let rename = buttons.take_right(button_width(&mut ui.gfx, "Rename", None).max(88.0));
        buttons.take_right(space::S);
        let replace = buttons.take_right(button_width(&mut ui.gfx, replace_label, None));
        buttons.take_right(space::S);
        let skip = buttons.take_right(button_width(&mut ui.gfx, "Skip", None));
        let mut answer = None;
        if ui
            .button(Id::new("sftp.clash.skip"), skip, "Skip", None, Kind::Ghost)
            .clicked
            || ui.key(Key::Escape, Modifiers::NONE)
        {
            answer = Some(Answer::Skip);
        }
        if ui
            .button(
                Id::new("sftp.clash.replace"),
                replace,
                replace_label,
                None,
                Kind::Secondary,
            )
            .clicked
        {
            answer = Some(Answer::Replace);
        }
        if clash.folder {
            ui.tooltip(
                Id::new("sftp.clash.replace"),
                replace,
                "Copies into the folder there; files of the same name are replaced",
            );
        }
        let renamed = ui
            .button(
                Id::new("sftp.clash.rename"),
                rename,
                "Rename",
                None,
                Kind::Primary,
            )
            .clicked
            || field.submitted;
        if renamed && !bad {
            answer = Some(Answer::Rename(typed));
        }
        pages::card_done(ui);
        if let Some(answer) = answer {
            self.answer_clash(&answer);
        }
    }

    /// The Delete, Permissions and Properties cards over the tab.
    pub(super) fn dialog_card(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        /// How long a card takes to leave, in seconds.
        const GOES: f64 = 0.16;
        let cwd = self.tab.cwd.clone().unwrap_or_default();
        if let Some((dialog, since)) = &mut self.card_left {
            #[allow(clippy::cast_possible_truncation)] // A share for drawing.
            let share = ((ui.time() - *since) / GOES) as f32;
            if share < 1.0 && !ui.state.reduce_motion {
                ui.push_inert();
                Self::dialog_shown(
                    ui,
                    rect,
                    dialog,
                    (&cwd, &self.tab.owners),
                    false,
                    Some(1.0 - Curve::Exit.at(share)),
                );
                ui.pop_inert();
                ui.request_repaint();
            } else {
                self.card_left = None;
            }
        }
        let Some(dialog) = &mut self.tab.dialog else {
            return;
        };
        let focus = std::mem::take(&mut self.focus_card);
        let (act, mut close) =
            Self::dialog_shown(ui, rect, dialog, (&cwd, &self.tab.owners), focus, None);
        match act {
            Some(Act::Send(command)) => {
                self.tab.send(command);
                self.remote_selected = None;
                close = true;
            }
            Some(Act::DeleteLocal(path)) => {
                let folder = matches!(
                    self.tab.dialog,
                    Some(Dialog::DeleteLocal { folder: true, .. })
                );
                self.tab
                    .local
                    .delete(path, folder, self.tab.show_hidden, self.waker.as_ref());
                self.local_selected = None;
                close = true;
            }
            None => {}
        }
        if close {
            self.card_left = self.tab.dialog.take().map(|dialog| (dialog, ui.time()));
        }
    }

    /// `dialog`'s card over `rect`, coming in, or at `leaving` of itself on its way out;
    /// `(cwd, owners)` are the host's folder shown and its names. Returns what its button
    /// asks for and whether it closes.
    #[allow(clippy::too_many_lines)] // Five cards, each top to bottom.
    fn dialog_shown(
        ui: &mut Ui<'_>,
        rect: Rect,
        dialog: &mut Dialog,
        (cwd, owners): (&str, &Owners),
        focus: bool,
        leaving: Option<f32>,
    ) -> (Option<Act>, bool) {
        let t = ui.theme;
        let facts = match dialog {
            Dialog::Properties(entry) => properties(cwd, entry, owners),
            Dialog::LocalProperties(lines) => lines.clone(),
            _ => Vec::new(),
        };
        #[allow(clippy::cast_precision_loss)] // A handful of lines.
        let height = match dialog {
            Dialog::Delete(_) | Dialog::DeleteLocal { .. } => 200.0,
            Dialog::Permissions { .. } => 214.0,
            Dialog::Properties(_) | Dialog::LocalProperties(_) => 136.0 + facts.len() as f32 * 24.0,
        };
        ui.zone(Id::new("sftp.card.backdrop"), rect, Role::Backdrop, "");
        let card = rect.centered(rect.w.min(480.0), height);
        if let Some(left) = leaving {
            ui.gfx.fill_rect(rect, 0.0, t.scrim.alpha(left));
            ui.push_layer(
                Layer::fade(left)
                    .scaled(card.center(), 0.98 + 0.02 * left)
                    .grouped(card.outset(48.0)),
            );
        } else {
            pages::card_in(ui, Id::new("sftp.card"), rect, card);
        }
        ui.gfx.shadow(card, 14.0, 28.0, 10.0, t.shadow);
        ui.gfx.fill_rect(card, 14.0, t.raised);
        ui.gfx.stroke_rect(card, 14.0, 1.0, t.line);
        let mut column = card.inset(space::XL, space::XL);
        let mut buttons = Rect::new(column.x, column.b() - CONTROL, column.w, CONTROL);
        let mut act = None;
        let mut close = ui.key(Key::Escape, Modifiers::NONE);
        match dialog {
            Dialog::Delete(_) | Dialog::DeleteLocal { .. } => {
                let (title, name, folder) = match dialog {
                    Dialog::Delete(entry) => {
                        ("Delete from the host?", entry.name.clone(), entry.is_dir())
                    }
                    Dialog::DeleteLocal { path, folder } => {
                        ("Delete from this computer?", file_name(path), *folder)
                    }
                    _ => unreachable!("matched above"),
                };
                ui.strict_text(column.take_top(24.0), title, HEADING, t.text, Align::Start);
                column.take_top(space::S);
                ui.gfx.text(
                    column.take_top(20.0),
                    &name,
                    Font::mono(13.0),
                    t.text,
                    Align::Start,
                );
                column.take_top(space::XS);
                let warning = if folder {
                    "The folder goes with everything in it. This cannot be undone."
                } else {
                    "This cannot be undone."
                };
                ui.gfx
                    .text(column.take_top(20.0), warning, SMALL, t.text2, Align::Start);
                let delete =
                    buttons.take_right(button_width(&mut ui.gfx, "Delete", None).max(88.0));
                buttons.take_right(space::S);
                let cancel = buttons.take_right(button_width(&mut ui.gfx, "Cancel", None));
                if focus {
                    // The safe choice has the keyboard.
                    ui.state.set_focus(Some(Id::new("sftp.card.cancel")), true);
                }
                close |= ui
                    .button(
                        Id::new("sftp.card.cancel"),
                        cancel,
                        "Cancel",
                        None,
                        Kind::Ghost,
                    )
                    .clicked;
                if ui
                    .button(
                        Id::new("sftp.card.delete"),
                        delete,
                        "Delete",
                        None,
                        Kind::Danger,
                    )
                    .clicked
                {
                    act = Some(match dialog {
                        Dialog::Delete(entry) => Act::Send(Command::Remove(join(cwd, &entry.name))),
                        Dialog::DeleteLocal { path, .. } => Act::DeleteLocal(path.clone()),
                        _ => unreachable!("matched above"),
                    });
                }
            }
            Dialog::Permissions { name, mode } => {
                ui.strict_text(
                    column.take_top(24.0),
                    "Permissions",
                    HEADING,
                    t.text,
                    Align::Start,
                );
                column.take_top(space::S);
                ui.gfx.text(
                    column.take_top(20.0),
                    name,
                    Font::mono(13.0),
                    t.text,
                    Align::Start,
                );
                column.take_top(space::S);
                let field_id = Id::new("sftp.card.mode");
                if focus {
                    ui.state.set_focus(Some(field_id), false);
                }
                let parsed = parse_mode(mode);
                let invalid = parsed.is_none() && !mode.trim().is_empty();
                let field = ui.text_field(
                    field_id,
                    column.take_top(CONTROL),
                    mode,
                    FieldStyle {
                        placeholder: "Octal, for example 644",
                        mono: true,
                        error: invalid,
                        ..FieldStyle::default()
                    },
                );
                // As typed this frame.
                let parsed = parse_mode(mode);
                if invalid {
                    column.take_top(space::XS);
                    ui.gfx.text(
                        column.take_top(18.0),
                        "Three or four octal digits, 0 to 7.",
                        SMALL,
                        t.error,
                        Align::Start,
                    );
                }
                let set = buttons.take_right(button_width(&mut ui.gfx, "Set", None).max(88.0));
                buttons.take_right(space::S);
                let cancel = buttons.take_right(button_width(&mut ui.gfx, "Cancel", None));
                close |= ui
                    .button(
                        Id::new("sftp.card.cancel"),
                        cancel,
                        "Cancel",
                        None,
                        Kind::Ghost,
                    )
                    .clicked;
                let chosen = ui
                    .button(Id::new("sftp.card.set"), set, "Set", None, Kind::Primary)
                    .clicked
                    || field.submitted;
                if chosen && let Some(bits) = parsed {
                    act = Some(Act::Send(Command::Chmod(join(cwd, name), bits)));
                }
            }
            Dialog::Properties(_) | Dialog::LocalProperties(_) => {
                ui.strict_text(
                    column.take_top(24.0),
                    "Properties",
                    HEADING,
                    t.text,
                    Align::Start,
                );
                column.take_top(space::S);
                for (fact, value) in &facts {
                    let mut line = column.take_top(24.0);
                    let label = line.take_left(110.0);
                    ui.gfx.text(label, fact, SMALL, t.text2, Align::Start);
                    let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, value, BODY, line.w);
                    ui.gfx.text(line, &fitted, BODY, t.text, Align::Start);
                }
                let done = buttons.take_right(button_width(&mut ui.gfx, "Close", None).max(88.0));
                if focus {
                    ui.state.set_focus(Some(Id::new("sftp.card.close")), true);
                }
                close |= ui
                    .button(
                        Id::new("sftp.card.close"),
                        done,
                        "Close",
                        None,
                        Kind::Secondary,
                    )
                    .clicked;
            }
        }
        pages::card_done(ui);
        (act, close)
    }

    fn pane_frame(ui: &mut Ui<'_>, rect: Rect, title: &str, path: &str, icon: IconId) -> Rect {
        let t = ui.theme;
        ui.card_surface(rect, 12.0);
        let mut inner = rect.inset(space::M, space::S);
        let mut head = inner.take_top(28.0);
        let mark = head.take_left(16.0).centered(16.0, 16.0);
        ui.gfx.icon(icon, mark, t.text2);
        head.take_left(space::S);
        let width = ui.gfx.measure(title, BODY_STRONG).width;
        ui.gfx.text(
            head.take_left(width + 4.0),
            title,
            BODY_STRONG,
            t.text,
            Align::Start,
        );
        head.take_left(space::S);
        // A deep folder keeps where it starts and where it ends.
        let shown = annulus_ui::gfx::fit_path(&mut ui.gfx, path, Font::mono(11.5), head.w);
        ui.gfx
            .text(head, &shown, Font::mono(11.5), t.text2, Align::Start);
        inner.take_top(4.0);
        inner
    }

    /// The listing's header, its rows' empty space and its rows for `side`: `rows` sorted and
    /// filtered already. Returns, for the row clicked, double-clicked, acted on or whose menu
    /// item was chosen, what happened, and the empty space's menu item chosen.
    #[allow(clippy::too_many_lines)] // The header, the empty space, then the rows.
    fn listing(
        &mut self,
        ui: &mut Ui<'_>,
        side: Side,
        area: Rect,
        rows: &[Row],
        empty: Option<&str>,
    ) -> Listed {
        let t = ui.theme;
        let available = available_columns(side);
        let (shown, sort) = match side {
            Side::Local => (&mut self.local_columns, &mut self.local_sort),
            Side::Remote => (&mut self.remote_columns, &mut self.tab.sort),
        };
        let header_line = Rect::new(area.x, area.y, area.w, 22.0);
        // As wide as the rows, which leave room for the scroll bar, so the columns line up.
        let cells_w = header_line.w - 10.0 - 2.0 * space::S;
        let laid = lay_out(shown, &available, cells_w);
        let changed = Self::header_row(ui, side, header_line, &laid, sort, shown, &available);
        if changed {
            self.columns_changed = true;
        }
        let mut list = area;
        list.take_top(22.0);
        let selected = match side {
            Side::Local => self.local_selected.clone(),
            Side::Remote => self.remote_selected.clone(),
        };
        let mut listed = Listed::default();
        // The empty space under and between the rows: a right click offers what the folder
        // does. Drawn first, so the rows over it take their own clicks.
        let blank_id = Id::new(("sftp.blank", side == Side::Local));
        let blank = ui.zone(blank_id, list, Role::ScrollView, "");
        if blank.clicked {
            match side {
                Side::Local => self.local_selected = None,
                Side::Remote => self.remote_selected = None,
            }
        }
        let blank_menu = folder_menu(side, self.tab.show_hidden);
        if let Some(choice) = ui.context_menu(blank_id, &blank, &blank_menu) {
            listed.folder_choice = blank_menu.get(choice).cloned();
        }
        if let Some(why) = empty {
            let line = Rect::new(list.x, list.y + space::XL, list.w, 20.0);
            ui.gfx.text(line, why, BODY, t.text2, Align::Center);
        }
        #[allow(clippy::cast_precision_loss)] // Folders hold thousands at most.
        let content = rows.len() as f32 * ROW;
        let scroll_id = match side {
            Side::Local => Id::new("sftp.local.scroll"),
            Side::Remote => Id::new("sftp.remote.scroll"),
        };
        let offset = ui.scroll_view(scroll_id, list, content);
        ui.push_list_clip(list, offset, content, FOCUS_REACH);
        let prefix = match side {
            Side::Local => "local",
            Side::Remote => "remote",
        };
        let action = match side {
            Side::Local => (icons::UPLOAD, "Upload to the host"),
            Side::Remote => (icons::DOWNLOAD, "Download to this computer"),
        };
        for (index, row) in rows.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let y = list.y - offset + index as f32 * ROW;
            if y + ROW < list.y || y > list.b() {
                continue;
            }
            let rect = Rect::new(list.x, y, list.w - 10.0, ROW);
            let id = Id::new((prefix, row.name.as_str()));
            let menu = row_menu(side, row.is_dir());
            let (clicked, double, acted, chosen) = Self::file_row(
                ui,
                id,
                rect,
                row,
                &laid,
                selected.as_deref() == Some(row.name.as_str()),
                action,
                &menu,
            );
            if clicked || chosen.is_some() {
                listed.clicked = Some(row.name.clone());
            }
            if double {
                listed.double = Some(row.name.clone());
            }
            if acted {
                listed.acted = Some(row.name.clone());
            }
            if let Some(item) = chosen.and_then(|at| menu.get(at)) {
                listed.choice = Some((row.name.clone(), item.clone()));
            }
            if let Some(at) = ui.drag_moved(id) {
                listed.drag = Some((row.name.clone(), at));
            }
            if let Some(at) = ui.drag_released(id) {
                listed.released = Some((row.name.clone(), at));
            }
        }
        ui.pop_clip();
        if let Some(name) = &listed.clicked {
            self.active = side;
            match side {
                Side::Local => self.local_selected = Some(name.clone()),
                Side::Remote => self.remote_selected = Some(name.clone()),
            }
        }
        if blank.clicked || blank.secondary_clicked {
            self.active = side;
        }
        listed
    }

    /// The column titles over a listing: a click sorts by one (again: the other way round),
    /// and a right click on any offers to show or hide the columns `available` to the pane.
    /// Returns whether the shown columns changed.
    fn header_row(
        ui: &mut Ui<'_>,
        side: Side,
        line: Rect,
        laid: &[(Column, Rect)],
        sort: &mut (Column, bool),
        shown: &mut Vec<Column>,
        available: &[Column],
    ) -> bool {
        let t = ui.theme;
        let mut changed = false;
        // A column shown but cut for room says so: the window is too narrow for it now.
        let menu: Vec<String> = available
            .iter()
            .map(|column| {
                let title = column.title();
                match (
                    shown.contains(column),
                    laid.iter().any(|(c, _)| c == column),
                ) {
                    (true, true) => format!("Hide {title}"),
                    (true, false) => format!("Hide {title} (no room for it now)"),
                    (false, _) => format!("Show {title}"),
                }
            })
            .collect();
        let origin = Rect::new(line.x + space::S, line.y, 0.0, line.h);
        for &(column, cell) in laid {
            let cell = Rect::new(origin.x + cell.x, line.y, cell.w, line.h);
            if cell.w <= 0.0 {
                continue;
            }
            let title = column.title();
            let align = column_align(column);
            let width = ui.gfx.measure(title, SMALL).width;
            let mut color = t.text3.mix(t.text2, 0.5);
            let id = Id::new(("sftp.sort", side == Side::Local, title));
            let response = ui.zone(
                id,
                cell,
                Role::Button,
                &format!("Sort by {}", title.to_lowercase()),
            );
            if response.clicked {
                *sort = (column, sort.0 != column || !sort.1);
            }
            if let Some(choice) = ui.context_menu(id, &response, &menu)
                && let Some(column) = available.get(choice)
            {
                if let Some(at) = shown.iter().position(|c| c == column) {
                    shown.remove(at);
                } else {
                    // What was asked for last is kept first when room runs short.
                    shown.insert(0, *column);
                }
                changed = true;
            }
            if response.focus_ring {
                ui.focus_ring(cell, 4.0);
            }
            if response.hovered {
                color = t.text;
                ui.set_cursor(annulus_ui::kit::Cursor::Pointer);
            }
            if sort.0 == column {
                color = t.text;
                let x = match align {
                    Align::End => cell.r() - width - 14.0,
                    _ => cell.x + width + 4.0,
                };
                let arrow = Rect::new(x, cell.center().y - 5.0, 10.0, 10.0);
                let icon = if sort.1 {
                    icons::CHEVRON_UP
                } else {
                    icons::CHEVRON_DOWN
                };
                ui.gfx.icon(icon, arrow, color);
            }
            let text_cell = match align {
                Align::End if sort.0 == column => Rect::new(cell.x, cell.y, cell.w - 14.0, cell.h),
                _ => cell,
            };
            let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, title, SMALL, text_cell.w);
            ui.gfx.text(text_cell, &fitted, SMALL, color, align);
        }
        ui.gfx
            .fill_rect(Rect::new(line.x, line.b() - 1.0, line.w, 1.0), 0.0, t.line);
        changed
    }

    /// One file row with its columns as `laid` out; returns whether it was clicked,
    /// double-clicked, its action button clicked, and the item of its menu (More, or a right
    /// click) chosen. Its buttons come up over its last columns while it is hovered.
    #[allow(clippy::too_many_arguments)] // One row's every part.
    fn file_row(
        ui: &mut Ui<'_>,
        id: Id,
        rect: Rect,
        row: &Row,
        laid: &[(Column, Rect)],
        selected: bool,
        action: (IconId, &str),
        menu: &[String],
    ) -> (bool, bool, bool, Option<usize>) {
        let t = ui.theme;
        let response = ui.zone(id, rect, Role::ListItem, &row.name);
        // Still hovered on its own buttons, and while one of its menus is open.
        let hover = ui.hover_within(rect) || ui.menu_open(id.with("more")) || ui.menu_open(id);
        let lit = ui.ease(id.with("lit"), if hover { 1.0 } else { 0.0 });
        let picked = ui.ease(id.with("picked"), if selected { 1.0 } else { 0.0 });
        let veils = |ui: &mut Ui<'_>, area: Rect| {
            ui.gfx
                .fill_rect(area, 6.0, t.hover.alpha(lit * (1.0 - picked)));
            ui.gfx.fill_rect(area, 6.0, t.selected.alpha(picked));
        };
        veils(ui, rect);
        if response.focus_ring {
            ui.focus_ring(rect, 6.0);
        }
        let origin_x = rect.x + space::S;
        for &(column, cell) in laid {
            let cell = Rect::new(origin_x + cell.x, rect.y, cell.w, rect.h);
            if column == Column::Name {
                let mut cell = cell;
                let icon = cell.take_left(16.0).centered(16.0, 16.0);
                let glyph = match row.kind {
                    annulus_sftp::Kind::Dir | annulus_sftp::Kind::Link { to_dir: true } => {
                        icons::FOLDER
                    }
                    _ => icons::FILE,
                };
                ui.gfx.icon(glyph, icon, t.text2);
                cell.take_left(space::S);
                // A long name keeps its end: "backup-2026…tar.gz".
                let shown = annulus_ui::gfx::fit_middle(&mut ui.gfx, &row.name, BODY, cell.w);
                ui.gfx.text(cell, &shown, BODY, t.text, Align::Start);
                continue;
            }
            let text = row.cell(column);
            let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, &text, SMALL, cell.w);
            ui.gfx
                .text(cell, &fitted, SMALL, t.text2, column_align(column));
        }
        let mut acted = false;
        let mut chosen = None;
        // They come up with the row, and stay for the moment it takes them to go.
        let near = lit.max(picked);
        if hover || selected || near > 0.0 {
            let mut end = rect.inset(space::S, 0.0);
            let more = end.take_right(26.0).centered(26.0, 26.0);
            end.take_right(space::XS);
            let button = end.take_right(26.0).centered(26.0, 26.0);
            ui.push_layer(Layer::fade(near));
            // Over the last columns, on the row's own color, so their text does not show
            // through.
            let backing = Rect::new(
                button.x - space::S,
                rect.y,
                rect.r() - button.x + space::S,
                rect.h,
            );
            ui.gfx.fill_rect(backing, 6.0, t.surface);
            veils(ui, backing);
            if !row.is_dir() {
                acted = ui
                    .icon_button(id.with("act"), button, action.0, action.1, false)
                    .clicked;
            }
            if !menu.is_empty() {
                chosen = ui.menu_button(
                    id.with("more"),
                    more,
                    "",
                    Some(icons::MORE),
                    Kind::Ghost,
                    menu,
                );
                ui.set_name(id.with("more"), "More actions");
                ui.tooltip(id.with("more"), more, "More actions");
            }
            ui.pop_layer();
        }
        // The same menu on a right click, or Shift+F10 on the row.
        if !menu.is_empty()
            && let Some(choice) = ui.context_menu(id, &response, menu)
        {
            chosen = Some(choice);
        }
        (response.clicked, response.double_clicked, acted, chosen)
    }

    /// The inline field for a new folder's name or a new name, over `side`'s listing; Enter or
    /// Save makes it so.
    fn edit_line(&mut self, ui: &mut Ui<'_>, side: Side, inner: &mut Rect) {
        // A rename typed for one folder never applies to the next one (UI review #9).
        if self.edit_dir != self.folder_of(side) && self.edit.as_ref().is_some_and(|e| e.0 == side)
        {
            self.edit = None;
        }
        let Some((_, edit)) = self.edit.as_mut().filter(|(at, _)| *at == side) else {
            return;
        };
        let mut line = inner.take_top(CONTROL);
        let save = line.take_right(80.0);
        line.take_right(space::S);
        let (buffer, placeholder) = match edit {
            Edit::NewFolder(name) => (name, "Folder name"),
            Edit::Rename { to, .. } => (to, "New name"),
        };
        let field_id = Id::new(("sftp.edit", side == Side::Local));
        let field = ui.text_field(
            field_id,
            line,
            buffer,
            FieldStyle {
                placeholder,
                ..FieldStyle::default()
            },
        );
        let done = ui
            .button(
                Id::new(("sftp.edit.save", side == Side::Local)),
                save,
                "Save",
                None,
                Kind::Primary,
            )
            .clicked
            || field.submitted;
        inner.take_top(space::S);
        if done && valid_name(buffer.trim()) {
            let Some((side, edit)) = self.edit.take() else {
                return;
            };
            match side {
                Side::Remote => {
                    let cwd = self.tab.cwd.clone().unwrap_or_default();
                    let command = match edit {
                        Edit::NewFolder(name) => Command::CreateDir(join(&cwd, name.trim())),
                        Edit::Rename { from, to } => {
                            Command::Rename(join(&cwd, &from), join(&cwd, to.trim()))
                        }
                    };
                    self.tab.send(command);
                }
                Side::Local => match edit {
                    Edit::NewFolder(name) => self.tab.local.create_dir(
                        name.trim().to_owned(),
                        self.tab.show_hidden,
                        self.waker.as_ref(),
                    ),
                    Edit::Rename { from, to } => self.tab.local.rename(
                        from,
                        to.trim().to_owned(),
                        self.tab.show_hidden,
                        self.waker.as_ref(),
                    ),
                },
            }
        } else if ui.key(Key::Escape, Modifiers::NONE) {
            self.edit = None;
        }
    }

    #[allow(clippy::too_many_lines)] // One pane, top to bottom.
    pub(super) fn local_pane(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let dir = self.tab.local.dir.clone();
        let home = home_dir();
        let mut inner = Self::pane_frame(
            ui,
            rect,
            "This computer",
            &tilde(&dir.display().to_string(), &home),
            icons::DEVICE,
        );
        let mut bar = inner.take_top(CONTROL);
        let up = bar.take_left(CONTROL);
        bar.take_left(4.0);
        let make = bar.take_left(CONTROL);
        if ui
            .icon_button(
                Id::new("sftp.local.up"),
                up,
                icons::CHEVRON_UP,
                "Up one folder",
                false,
            )
            .clicked
            && let Some(parent) = dir.parent()
        {
            self.tab.local.navigate(
                parent.to_path_buf(),
                self.tab.show_hidden,
                self.waker.as_ref(),
            );
            self.local_selected = None;
        }
        if ui
            .icon_button(
                Id::new("sftp.local.mkdir"),
                make,
                icons::PLUS,
                "New folder on this computer",
                false,
            )
            .clicked
        {
            self.start_edit(Side::Local, None);
        }
        ui.tooltip(Id::new("sftp.local.mkdir"), make, "New folder");
        // A filter like the host's pane has, so the two lists line up.
        inner.take_top(space::S);
        ui.text_field(
            Id::new("sftp.local.filter"),
            inner.take_top(CONTROL),
            &mut self.local_filter,
            FieldStyle {
                placeholder: "Filter this folder",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        inner.take_top(space::S);
        self.edit_line(ui, Side::Local, &mut inner);
        let filter = self.local_filter.trim().to_lowercase();
        let read_error = self.tab.local.error.clone();
        let mut rows: Vec<Row> = self
            .tab
            .local
            .entries
            .iter()
            .map(crate::local_pane::LocalEntry::row)
            .filter(|row| filter.is_empty() || row.name.to_lowercase().contains(&filter))
            .collect();
        sort_rows(&mut rows, self.local_sort.0, self.local_sort.1);
        let empty = if self.tab.local.loading {
            Some("Loading this folder…".to_owned())
        } else if let Some(error) = &read_error {
            Some(format!("Cannot read this folder: {error}"))
        } else if rows.is_empty() && !filter.is_empty() {
            Some(format!(
                "Nothing here matches \u{201c}{}\u{201d}.",
                self.local_filter.trim()
            ))
        } else if rows.is_empty() {
            Some("This folder is empty.".to_owned())
        } else {
            None
        };
        let listed = self.listing(ui, Side::Local, inner, &rows, empty.as_deref());
        let shown_dir = self.tab.local.dir.clone();
        let path_of = |name: &str| shown_dir.join(name);
        let is_dir = |name: &str| rows.iter().any(|row| row.name == name && row.is_dir());
        if let Some((name, at)) = &listed.drag {
            self.drag = Some((Dragged::Local(path_of(name)), *at));
        }
        if let Some((name, at)) = &listed.released {
            self.dropped = Some((Dragged::Local(path_of(name)), *at));
        }
        // A folder opens; a file goes up.
        if let Some(name) = &listed.double {
            if is_dir(name) {
                self.tab
                    .local
                    .navigate(path_of(name), self.tab.show_hidden, self.waker.as_ref());
                self.local_selected = None;
            } else {
                self.send_up(path_of(name));
            }
        }
        if let Some(name) = &listed.acted {
            self.send_up(path_of(name));
        }
        if let Some((name, item)) = listed.choice {
            let path = path_of(&name);
            match item.as_str() {
                OPEN if is_dir(&name) => {
                    self.tab
                        .local
                        .navigate(path, self.tab.show_hidden, self.waker.as_ref());
                    self.local_selected = None;
                }
                OPEN => {
                    if let Err(error) = platform::open_path(&path) {
                        ui.toast(format!("Could not open {name}: {error}"), Tone::Problem);
                    }
                }
                UPLOAD => self.send_up(path),
                RENAME => self.start_edit(Side::Local, Some(name)),
                COPY_PATH => ui.copy(path.display().to_string()),
                PROPERTIES => {
                    if let Some(row) = rows.iter().find(|row| row.name == name) {
                        let lines = local_properties(&path, row);
                        self.open_card(Dialog::LocalProperties(lines));
                    }
                }
                NEW_FOLDER => self.start_edit(Side::Local, None),
                REFRESH => self
                    .tab
                    .local
                    .refresh(self.tab.show_hidden, self.waker.as_ref()),
                DELETE => self.ask_delete_local(&name),
                _ if item.starts_with(SHOW_IN) => {
                    if let Err(error) = platform::reveal_path(&path) {
                        ui.toast(format!("Could not show {name}: {error}"), Tone::Problem);
                    }
                }
                _ => {}
            }
        }
        if let Some(item) = listed.folder_choice {
            self.folder_action(ui, Side::Local, &item);
        }
    }

    /// What the empty space's menu item `item` does in `side`'s folder.
    fn folder_action(&mut self, ui: &mut Ui<'_>, side: Side, item: &str) {
        match item {
            NEW_FOLDER => self.start_edit(side, None),
            REFRESH => match side {
                Side::Local => self
                    .tab
                    .local
                    .refresh(self.tab.show_hidden, self.waker.as_ref()),
                Side::Remote => self.tab.reload(),
            },
            COPY_PATH => {
                let path = self.folder_of(side);
                ui.copy(path);
            }
            SHOW_HIDDEN | HIDE_HIDDEN => {
                self.tab.show_hidden = !self.tab.show_hidden;
                self.tab.reload();
                self.tab
                    .local
                    .refresh(self.tab.show_hidden, self.waker.as_ref());
            }
            _ if item.starts_with(SHOW_IN) => {
                if let Err(error) = platform::open_path(&self.tab.local.dir) {
                    ui.toast(format!("Could not open the folder: {error}"), Tone::Problem);
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_lines)] // Toolbar, an inline edit and the list.
    pub(super) fn remote_pane(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let t = ui.theme;
        let cwd = self.tab.cwd.clone().unwrap_or_default();
        let title = self.tab.title.clone();
        let shown = tilde(&cwd, &self.tab.home);
        let mut inner = Self::pane_frame(ui, rect, &title, &shown, icons::SERVER);
        let mut bar = inner.take_top(CONTROL);
        let up = bar.take_left(CONTROL);
        bar.take_left(4.0);
        let refresh = bar.take_left(CONTROL);
        bar.take_left(4.0);
        let folder_w = button_width(&mut ui.gfx, "New folder", Some(icons::PLUS));
        let hidden_w = button_width(&mut ui.gfx, "Hidden files", None);
        // A narrow pane keeps New folder as an icon, so neither button is cut off.
        let roomy = bar.w >= folder_w + hidden_w + space::S;
        let new_folder = bar.take_left(if roomy { folder_w } else { CONTROL });
        let hidden = bar.take_right(hidden_w);
        if ui
            .icon_button(
                Id::new("sftp.up"),
                up,
                icons::CHEVRON_UP,
                "Up one folder",
                false,
            )
            .clicked
        {
            self.tab.navigate(parent(&cwd));
        }
        if ui
            .icon_button(
                Id::new("sftp.refresh"),
                refresh,
                icons::SYNC,
                "Refresh",
                false,
            )
            .clicked
        {
            self.tab.reload();
        }
        let make = if roomy {
            ui.button(
                Id::new("sftp.mkdir"),
                new_folder,
                "New folder",
                Some(icons::PLUS),
                Kind::Ghost,
            )
            .clicked
        } else {
            let clicked = ui
                .icon_button(
                    Id::new("sftp.mkdir"),
                    new_folder,
                    icons::PLUS,
                    "New folder",
                    false,
                )
                .clicked;
            ui.tooltip(Id::new("sftp.mkdir"), new_folder, "New folder");
            clicked
        };
        if make {
            self.start_edit(Side::Remote, None);
        }
        let kind = if self.tab.show_hidden {
            Kind::Secondary
        } else {
            Kind::Ghost
        };
        if ui
            .button(Id::new("sftp.hidden"), hidden, "Hidden files", None, kind)
            .clicked
        {
            self.tab.show_hidden = !self.tab.show_hidden;
            self.tab.reload();
            self.tab
                .local
                .refresh(self.tab.show_hidden, self.waker.as_ref());
        }
        inner.take_top(space::S);
        let mut options = inner.take_top(CONTROL);
        // Asking first, then the fixed answers.
        let policies = [
            None,
            Some(Conflict::KeepBoth),
            Some(Conflict::Overwrite),
            Some(Conflict::Skip),
        ];
        let names: Vec<String> = policies
            .iter()
            .map(|policy| policy.map_or(ASK_NAME, conflict_name).to_owned())
            .collect();
        let widest = names
            .iter()
            .map(|name| ui.gfx.measure(name, BODY).width)
            .fold(0.0, f32::max);
        let conflict = options.take_right(widest + 44.0);
        options.take_right(space::S);
        ui.text_field(
            Id::new("sftp.filter"),
            options,
            &mut self.tab.filter,
            FieldStyle {
                placeholder: "Filter this folder",
                icon: Some(icons::SEARCH),
                ..FieldStyle::default()
            },
        );
        let current = (!self.tab.ask_on_clash).then_some(self.tab.conflict);
        let at = policies.iter().position(|policy| *policy == current);
        if let Some(chosen) = ui.select(
            Id::new("sftp.conflict"),
            conflict,
            &names,
            at,
            "Existing files",
        ) {
            match policies[chosen] {
                None => self.tab.ask_on_clash = true,
                Some(policy) => {
                    self.tab.ask_on_clash = false;
                    self.tab.conflict = policy;
                }
            }
        }
        inner.take_top(space::S);
        self.edit_line(ui, Side::Remote, &mut inner);
        if let Some(message) = self
            .tab
            .list_error
            .clone()
            .or_else(|| self.tab.notice.clone())
        {
            ui.gfx
                .text(inner.take_top(20.0), &message, SMALL, t.error, Align::Start);
        }
        let filter = self.tab.filter.trim().to_lowercase();
        let mut rows: Vec<Row> = self
            .tab
            .entries
            .iter()
            .filter(|entry| self.tab.show_hidden || !entry.name.starts_with('.'))
            .filter(|entry| filter.is_empty() || entry.name.to_lowercase().contains(&filter))
            .map(|entry| Row::remote(entry, &self.tab.owners))
            .collect();
        sort_rows(&mut rows, self.tab.sort.0, self.tab.sort.1);
        // An empty listing says why: nothing there, only hidden files, or nothing matches.
        let empty = (rows.is_empty() && self.tab.cwd.is_some()).then(|| {
            if !filter.is_empty() {
                format!(
                    "Nothing here matches \u{201c}{}\u{201d}.",
                    self.tab.filter.trim()
                )
            } else if self.tab.entries.is_empty() {
                "This folder is empty.".to_owned()
            } else {
                "Only hidden files here; Hidden files shows them.".to_owned()
            }
        });
        let listed = self.listing(ui, Side::Remote, inner, &rows, empty.as_deref());
        let entries = self.tab.entries.clone();
        let entry_of = |name: &str| entries.iter().find(|entry| entry.name == name).cloned();
        if let Some((name, at)) = &listed.drag
            && let Some(entry) = entry_of(name)
        {
            self.drag = Some((Dragged::Remote(entry), *at));
        }
        if let Some((name, at)) = &listed.released
            && let Some(entry) = entry_of(name)
        {
            self.dropped = Some((Dragged::Remote(entry), *at));
        }
        if let Some(entry) = listed.double.as_deref().and_then(entry_of) {
            if entry.is_dir() {
                self.remote_selected = None;
                self.tab.navigate(join(&cwd, &entry.name));
            } else {
                self.send_down(entry);
            }
        }
        if let Some(entry) = listed.acted.as_deref().and_then(entry_of) {
            self.send_down(entry);
        }
        if let Some((name, item)) = listed.choice
            && let Some(entry) = entry_of(&name)
        {
            match item.as_str() {
                OPEN => {
                    self.remote_selected = None;
                    self.tab.navigate(join(&cwd, &entry.name));
                }
                DOWNLOAD => self.send_down(entry),
                RENAME => self.start_edit(Side::Remote, Some(entry.name)),
                PERMISSIONS => self.open_card(Dialog::permissions(&entry)),
                PROPERTIES => self.open_card(Dialog::Properties(entry)),
                COPY_PATH => ui.copy(join(&cwd, &entry.name)),
                NEW_FOLDER => self.start_edit(Side::Remote, None),
                REFRESH => self.tab.reload(),
                DELETE => self.open_card(Dialog::Delete(entry)),
                _ => {}
            }
        }
        if let Some(item) = listed.folder_choice {
            self.folder_action(ui, Side::Remote, &item);
        }
    }

    /// The transfers along the bottom (F-SFTP-004), newest first. Each row has the file's
    /// name and how it is doing, where it goes with the folders between shortened, and a bar
    /// that glides with the progress; Cancel or Retry at its end. The panel folds to its
    /// header, and Clear finished takes the done ones away.
    #[allow(clippy::too_many_lines)] // The header, then one row after another.
    pub(super) fn transfers(&mut self, ui: &mut Ui<'_>, rect: Rect) {
        let t = ui.theme;
        let mut area = rect;
        area.take_top(space::M);
        let mut head = area.take_top(TRANSFERS_HEAD);
        let running = self
            .tab
            .transfers
            .iter()
            .filter(|transfer| matches!(transfer.state, TransferState::Running))
            .count();
        let finished = self.tab.transfers.iter().any(|transfer| {
            matches!(
                transfer.state,
                TransferState::Done(_) | TransferState::Skipped
            )
        });
        // Laid out from the right, drawn after the toggle: Tab follows the drawing order.
        let clear_label = "Clear finished";
        let clear = finished.then(|| head.take_right(button_width(&mut ui.gfx, clear_label, None)));
        let summary = match running {
            0 => format!("Transfers \u{b7} {}", self.tab.transfers.len()),
            n => format!(
                "Transfers \u{b7} {n} running of {}",
                self.tab.transfers.len()
            ),
        };
        let toggle_w = ui.gfx.measure(&summary, BODY_STRONG).width + 16.0 + 3.0 * space::S;
        let toggle = head.take_left(toggle_w.min(head.w));
        let toggle_id = Id::new("sftp.transfers.toggle");
        let name = if self.transfers_open {
            "Hide transfers"
        } else {
            "Show transfers"
        };
        let response = ui.zone(toggle_id, toggle, Role::Button, name);
        if response.clicked {
            self.transfers_open = !self.transfers_open;
        }
        let hover = ui.ease(
            toggle_id.with("hover"),
            if response.hovered { 1.0 } else { 0.0 },
        );
        ui.gfx.fill_rect(toggle, 7.0, t.hover.alpha(hover));
        if response.focus_ring {
            ui.focus_ring(toggle, 7.0);
        }
        let mut inner = toggle.inset(space::S, 0.0);
        let chevron = if self.transfers_open {
            icons::CHEVRON_DOWN
        } else {
            icons::CHEVRON_RIGHT
        };
        ui.gfx
            .icon(chevron, inner.take_left(16.0).centered(14.0, 14.0), t.text2);
        inner.take_left(space::S);
        ui.gfx
            .text(inner, &summary, BODY_STRONG, t.text, Align::Start);
        if let Some(clear) = clear
            && ui
                .button(
                    Id::new("sftp.transfers.clear"),
                    clear,
                    clear_label,
                    None,
                    Kind::Ghost,
                )
                .clicked
        {
            self.tab.transfers.retain(|transfer| {
                !matches!(
                    transfer.state,
                    TransferState::Done(_) | TransferState::Skipped
                )
            });
        }
        area.take_top(space::S);
        if !self.transfers_open {
            return;
        }
        let rows = self.tab.transfers.len();
        #[allow(clippy::cast_precision_loss)] // A few dozen transfers.
        let content = rows as f32 * (TRANSFER_ROW + 6.0);
        let offset = ui.scroll_view(Id::new("sftp.transfers"), area, content);
        ui.push_clip(area);
        let home = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().display().to_string())
            .unwrap_or_default();
        let mut retry = None;
        let mut moving = false;
        for (place, transfer) in self.tab.transfers.iter().rev().enumerate() {
            #[allow(clippy::cast_precision_loss)] // As above.
            let y = area.y - offset + place as f32 * (TRANSFER_ROW + 6.0);
            let row = Rect::new(area.x, y, area.w - 10.0, TRANSFER_ROW);
            let id = Id::new(("transfer", transfer.id));
            // A new transfer grows into its place in the list.
            let enter = ui.entrance(id.with("enter"), Motion::POP);
            ui.push_layer(Layer::appear(enter, row.center(), 0.92, 0.0));
            ui.card_surface(row, 10.0);
            let mut cells = row.inset(space::M, 0.0);
            let (state_color, status) = transfer_status(transfer, &t);
            // Its direction, in a tile tinted by how it is doing.
            let tile = cells.take_left(32.0).centered(32.0, 32.0);
            let tint = ui.animate(id.with("tint"), state_tone(transfer), Motion::SLOW);
            let tone = t.accent.mix(t.ok, tint.clamp(0.0, 1.0));
            let tone = match transfer.state {
                TransferState::Failed(_) => t.error,
                TransferState::Skipped => t.text3,
                _ => tone,
            };
            ui.gfx.fill_rect(tile, 9.0, tone.alpha(0.16));
            let glyph = match transfer.state {
                TransferState::Done(_) => icons::CHECK,
                TransferState::Failed(_) => icons::WARNING,
                _ if transfer.upload => icons::UPLOAD,
                _ => icons::DOWNLOAD,
            };
            ui.gfx.icon(glyph, tile.centered(16.0, 16.0), tone);
            cells.take_left(space::M);
            let button = cells.take_right(28.0).centered(28.0, 28.0);
            cells.take_right(space::S);
            let name = transfer
                .label
                .trim_start_matches(['\u{2191}', '\u{2193}'])
                .trim();
            let status_w = ui.gfx.measure(&status, SMALL).width.min(cells.w * 0.45);
            let top = Rect::new(cells.x, row.y + 9.0, cells.w, 20.0);
            let mut name_cell = top;
            let status_cell = name_cell.take_right(status_w);
            name_cell.take_right(space::M);
            let fitted = annulus_ui::gfx::fit_middle(&mut ui.gfx, name, BODY_STRONG, name_cell.w);
            ui.gfx
                .text(name_cell, &fitted, BODY_STRONG, t.text, Align::Start);
            ui.gfx
                .text(status_cell, &status, SMALL, state_color, Align::End);
            let second = Rect::new(cells.x, row.y + 27.0, cells.w, 16.0);
            if let TransferState::Failed(message) = &transfer.state {
                ui.gfx.text(second, message, SMALL, t.error, Align::Start);
            } else {
                let route = format!("To {}", destination(transfer, &home));
                let fitted = annulus_ui::gfx::fit_path(&mut ui.gfx, &route, SMALL, second.w);
                ui.gfx.text(second, &fitted, SMALL, t.text3, Align::Start);
            }
            // The bar along the row's foot: it glides to each new share.
            let share = match transfer.state {
                TransferState::Done(_) | TransferState::Skipped => 1.0,
                _ => share_of(transfer),
            };
            let shown = ui.animate(
                id.with("bar"),
                share,
                Motion::new(450, annulus_ui::motion::Curve::Out),
            );
            let track = Rect::new(cells.x, row.b() - 10.0, cells.w, 3.0);
            let fill = Rect::new(track.x, track.y, track.w * shown.clamp(0.0, 1.0), track.h);
            // Only while it says something: a transfer that is over needs no bar.
            if !matches!(
                transfer.state,
                TransferState::Done(_) | TransferState::Skipped
            ) {
                ui.gfx.fill_rect(track, 1.5, t.hover);
                ui.gfx.fill_rect(fill, 1.5, tone);
            }
            if matches!(transfer.state, TransferState::Running) && fill.w > 4.0 {
                // A soft light running along what is done, so a slow transfer still looks alive.
                moving = true;
                let band = 48.0;
                #[allow(clippy::cast_possible_truncation)] // A phase for drawing.
                let phase = (ui.time() * 0.7).fract() as f32;
                let x = fill.x - band + phase * (fill.w + band);
                ui.push_clip(fill);
                ui.gfx.fill_rect(
                    Rect::new(x, fill.y, band, fill.h),
                    1.5,
                    Color::WHITE.alpha(0.35),
                );
                ui.pop_clip();
            }
            let running = matches!(transfer.state, TransferState::Running);
            let failed = matches!(transfer.state, TransferState::Failed(_));
            if running
                && ui
                    .icon_button(id, button, icons::CLOSE, "Cancel", false)
                    .clicked
            {
                transfer.cancel.store(true, Ordering::Relaxed);
            }
            if failed
                && ui
                    .icon_button(id, button, icons::SYNC, "Retry", false)
                    .clicked
            {
                retry = Some(transfer.id);
            }
            ui.pop_layer();
        }
        ui.pop_clip();
        if moving {
            ui.request_repaint_after(std::time::Duration::from_millis(33));
        }
        if let Some(id) = retry {
            self.tab.retry(id);
        }
    }
}
