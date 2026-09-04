use super::ram_package::read_le_u32;
use super::*;

const TEXT_VIEWER_BODY_TOP: i32 = 32;
const TEXT_VIEWER_LINE_HEIGHT: i32 = 22;
const TEXT_VIEWER_GLYPH_HEIGHT: i32 = 16;
const PLATFORM_SOFTKEY_HEIGHT: i32 = 26;

impl ExtRuntime {
    pub(super) fn create_platform_editor(
        &mut self,
        module: usize,
        title: Vec<u16>,
        text: Vec<u16>,
        editor_type: u32,
        max_code_units: usize,
    ) -> Result<u32> {
        if max_code_units > MAX_PLATFORM_EDITOR_CODE_UNITS {
            return Err(Error::ResourceLimit(format!(
                "platform editor requested {max_code_units} code units (limit {MAX_PLATFORM_EDITOR_CODE_UNITS})"
            )));
        }
        if text.len() > max_code_units {
            return Err(Error::Abi(format!(
                "platform editor initial text has {} code units (limit {max_code_units})",
                text.len()
            )));
        }
        let owner_generation = self
            .modules
            .get(module)
            .ok_or_else(|| Error::Abi(format!("editor creation for missing module {module}")))?
            .generation;
        let buffer_len = max_code_units
            .checked_add(1)
            .and_then(|units| units.checked_mul(2))
            .ok_or_else(|| Error::Abi("platform editor buffer length overflow".into()))?;
        let handle = self.allocate_ui_handle()?;
        let buffer = self
            .allocate_guest_block_for_module(buffer_len, module)?
            .ok_or_else(|| {
                Error::ResourceLimit("guest heap exhausted for platform editor".into())
            })?;
        self.memory.write(buffer, &vec![0; buffer_len])?;
        self.write_platform_editor_units(buffer, &text)?;
        self.editors.insert(
            handle,
            PlatformEditor {
                owner_generation,
                _title: title,
                _editor_type: editor_type,
                max_code_units,
                text,
                buffer,
                buffer_len,
            },
        );
        self.active_platform_ui
            .push(ActivePlatformUi::Editor(handle));
        Ok(handle)
    }

    pub(super) fn release_platform_editor(&mut self, module: usize, handle: u32) -> Result<bool> {
        let owner_generation = self
            .modules
            .get(module)
            .ok_or_else(|| Error::Abi(format!("editor release for missing module {module}")))?
            .generation;
        let Some(editor) = self.editors.get(&handle) else {
            return Ok(false);
        };
        if editor.owner_generation != owner_generation {
            return Ok(false);
        }
        let (buffer, buffer_len) = (editor.buffer, editor.buffer_len);
        self.free_guest_block_for_module(buffer, buffer_len, module)?;
        self.editors.remove(&handle);
        let ui = ActivePlatformUi::Editor(handle);
        self.active_platform_ui.retain(|active| *active != ui);
        if self
            .platform_pointer_capture
            .is_some_and(|capture| capture.ui == ui)
        {
            self.platform_pointer_capture = None;
        }
        Ok(true)
    }

    pub(super) fn platform_editor_text(
        &self,
        module: usize,
        handle: u32,
    ) -> Result<Option<GuestAddr>> {
        let owner_generation = self
            .modules
            .get(module)
            .ok_or_else(|| Error::Abi(format!("editor access for missing module {module}")))?
            .generation;
        Ok(self
            .editors
            .get(&handle)
            .filter(|editor| editor.owner_generation == owner_generation)
            .map(|editor| editor.buffer))
    }

    pub(super) fn set_platform_editor_text(&mut self, handle: u32, text: &str) -> Result<()> {
        let editor = self.editors.get(&handle).ok_or_else(|| {
            Error::Abi(format!("active platform editor handle {handle} is missing"))
        })?;
        let (buffer, max_code_units) = (editor.buffer, editor.max_code_units);
        let mut units = Vec::with_capacity(text.len().min(max_code_units));
        for character in text.chars() {
            let required = character.len_utf16();
            if units.len().saturating_add(required) > max_code_units {
                break;
            }
            let mut encoded = [0_u16; 2];
            units.extend_from_slice(character.encode_utf16(&mut encoded));
        }
        self.write_platform_editor_units(buffer, &units)?;
        self.editors
            .get_mut(&handle)
            .expect("validated platform editor remains live")
            .text = units;
        Ok(())
    }

    fn write_platform_editor_units(&mut self, buffer: GuestAddr, units: &[u16]) -> Result<()> {
        let mut encoded = Vec::with_capacity((units.len() + 1) * 2);
        for unit in units {
            encoded.extend_from_slice(&unit.to_be_bytes());
        }
        encoded.extend_from_slice(&[0, 0]);
        self.memory.write(buffer, &encoded)
    }

    pub(super) fn create_native_window(&mut self, module: usize) -> Result<u32> {
        let owner_generation = self
            .modules
            .get(module)
            .ok_or_else(|| Error::Abi(format!("window creation for missing module {module}")))?
            .generation;
        let handle = self.allocate_ui_handle()?;
        self.native_windows.insert(handle, owner_generation);
        Ok(handle)
    }

    pub(super) fn release_native_window(&mut self, module: usize, handle: u32) -> Result<bool> {
        let owner_generation = self
            .modules
            .get(module)
            .ok_or_else(|| Error::Abi(format!("window release for missing module {module}")))?
            .generation;
        if self.native_windows.get(&handle) != Some(&owner_generation) {
            return Ok(false);
        }
        self.native_windows.remove(&handle);
        Ok(true)
    }

    pub(super) fn create_platform_menu(
        &mut self,
        title: Vec<u16>,
        item_count: usize,
    ) -> Result<u32> {
        if item_count > MAX_PLATFORM_MENU_ITEMS {
            return Err(Error::ResourceLimit(format!(
                "platform menu requested {item_count} items (limit {MAX_PLATFORM_MENU_ITEMS})"
            )));
        }
        let handle = self.allocate_ui_handle()?;
        self.menus.insert(
            handle,
            PlatformMenu {
                title,
                items: vec![None; item_count],
                focused_item: 0,
                first_visible_item: 0,
                previous_screen: None,
                menu_screen: None,
                modal_detached: false,
            },
        );
        Ok(handle)
    }

    pub(super) fn set_platform_menu_item(
        &mut self,
        handle: u32,
        index: usize,
        text: Vec<u16>,
    ) -> bool {
        let Some(item) = self
            .menus
            .get_mut(&handle)
            .and_then(|menu| menu.items.get_mut(index))
        else {
            return false;
        };
        *item = Some(text);
        true
    }

    pub(super) fn selected_platform_menu_item(&self, handle: u32) -> Option<usize> {
        let menu = self.menus.get(&handle)?;
        menu.items
            .get(menu.focused_item)
            .and_then(Option::as_ref)
            .map(|_| menu.focused_item)
    }

    pub(super) fn set_platform_menu_focus(
        &mut self,
        handle: u32,
        index: usize,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        let Some(menu) = self.menus.get_mut(&handle) else {
            return Ok(false);
        };
        if menu.items.get(index).and_then(Option::as_ref).is_none() {
            return Ok(false);
        }
        menu.focused_item = index;
        self.render_platform_menu(handle, services)?;
        Ok(true)
    }

    pub(super) fn move_platform_menu_focus(
        &mut self,
        handle: u32,
        direction: i32,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        let Some(menu) = self.menus.get(&handle) else {
            return Ok(false);
        };
        let item_count = menu.items.len();
        if item_count == 0 {
            self.render_platform_menu(handle, services)?;
            return Ok(true);
        }
        let focused_item = menu.focused_item.min(item_count - 1);
        let next = (1..=item_count).find_map(|distance| {
            let index = if direction < 0 {
                (focused_item + item_count - distance % item_count) % item_count
            } else {
                (focused_item + distance) % item_count
            };
            menu.items[index].as_ref().map(|_| index)
        });
        if let Some(next) = next {
            self.menus
                .get_mut(&handle)
                .expect("menu handle was checked")
                .focused_item = next;
        }
        self.render_platform_menu(handle, services)?;
        Ok(true)
    }

    pub(super) fn show_platform_menu(
        &mut self,
        handle: u32,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        if !self.menus.contains_key(&handle) {
            return Ok(false);
        }
        self.pending_platform_menu_selection = None;
        let ui = ActivePlatformUi::Menu(handle);
        if let Some(position) = self
            .active_platform_ui
            .iter()
            .position(|active| *active == ui)
        {
            self.active_platform_ui.truncate(position + 1);
            self.platform_pointer_capture = None;
            self.render_platform_menu(handle, services)?;
            return Ok(true);
        }

        let previous_screen = self.capture_platform_screen(services)?;
        self.memory.write(self.screen_base, &previous_screen)?;
        self.menus
            .get_mut(&handle)
            .expect("menu handle was checked")
            .previous_screen = Some(previous_screen.clone());
        if let Err(error) = self.render_platform_menu(handle, services) {
            self.memory.write(self.screen_base, &previous_screen)?;
            self.menus
                .get_mut(&handle)
                .expect("menu handle was checked")
                .previous_screen = None;
            return Err(error);
        }
        self.active_platform_ui.push(ui);
        Ok(true)
    }

    pub(super) fn release_platform_menu(
        &mut self,
        handle: u32,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        if self.pending_platform_menu_selection == Some(handle) {
            self.pending_platform_menu_selection = None;
        }
        let Some(menu) = self.menus.remove(&handle) else {
            return Ok(false);
        };
        let ui = ActivePlatformUi::Menu(handle);
        let position = self
            .active_platform_ui
            .iter()
            .position(|active| *active == ui);
        let was_top =
            position.is_some_and(|position| position + 1 == self.active_platform_ui.len());
        let modal_parent_screen = (position.is_none() && menu.modal_detached)
            .then(|| {
                self.active_platform_ui
                    .iter()
                    .all(|ui| matches!(ui, ActivePlatformUi::Menu(_)))
            })
            .filter(|all_menus| *all_menus)
            .and_then(|_| {
                self.active_platform_ui.iter().find_map(|ui| {
                    let ActivePlatformUi::Menu(handle) = ui else {
                        return None;
                    };
                    self.menus
                        .get(handle)
                        .and_then(|menu| menu.previous_screen.clone())
                })
            });
        if let Some(position) = position {
            self.active_platform_ui.remove(position);
        }
        if self
            .platform_pointer_capture
            .is_some_and(|capture| capture.ui == ui)
        {
            self.platform_pointer_capture = None;
        }
        if let Some(previous_screen) = modal_parent_screen {
            self.pending_platform_menu_returns = self
                .pending_platform_menu_returns
                .checked_add(self.active_platform_ui.len())
                .filter(|count| *count <= MAX_PENDING_PLATFORM_MENU_RETURNS)
                .ok_or_else(|| {
                    Error::ResourceLimit("too many pending platform menu returns".into())
                })?;
            self.active_platform_ui.clear();
            self.platform_pointer_capture = None;
            self.memory.write(self.screen_base, &previous_screen)?;
            self.present_screen(services)?;
        } else if was_top && let Some(previous_screen) = menu.previous_screen {
            self.memory.write(self.screen_base, &previous_screen)?;
            self.present_screen(services)?;
        }
        Ok(true)
    }

    pub(super) fn refresh_platform_menu(
        &mut self,
        handle: u32,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        let ui = ActivePlatformUi::Menu(handle);
        if self.active_platform_ui.last() == Some(&ui) {
            if self.pending_platform_menu_selection == Some(handle) {
                self.pending_platform_menu_selection = None;
            }
            self.render_platform_menu(handle, services)?;
            return Ok(true);
        }
        if !self
            .menus
            .get(&handle)
            .is_some_and(|menu| menu.modal_detached)
            || !self
                .active_platform_ui
                .iter()
                .all(|active| matches!(active, ActivePlatformUi::Menu(_)))
        {
            return Ok(false);
        }
        if self.pending_platform_menu_selection == Some(handle) {
            self.pending_platform_menu_selection = None;
        }
        self.menus
            .get_mut(&handle)
            .expect("detached menu handle was checked")
            .modal_detached = false;
        self.active_platform_ui.push(ui);
        if let Err(error) = self.render_platform_menu(handle, services) {
            self.active_platform_ui.pop();
            self.menus
                .get_mut(&handle)
                .expect("detached menu handle remains live")
                .modal_detached = true;
            return Err(error);
        }
        Ok(true)
    }

    pub(super) fn platform_menu_pointer_action(
        &self,
        handle: u32,
        x: i32,
        y: i32,
    ) -> Result<PlatformPointerAction> {
        let Some(menu) = self.menus.get(&handle) else {
            return Ok(PlatformPointerAction::None);
        };
        let (width, height) = self.screen_dimensions()?;
        if x < 0 || y < 0 || x >= width || y >= height {
            return Ok(PlatformPointerAction::None);
        }
        let item_top = 34;
        let item_height = 24;
        let softkey_top = height.saturating_sub(26).max(item_top);
        if y >= softkey_top {
            if x >= width / 2 {
                return Ok(PlatformPointerAction::MenuReturn);
            }
            return Ok(self
                .selected_platform_menu_item(handle)
                .map(PlatformPointerAction::MenuSelect)
                .unwrap_or(PlatformPointerAction::None));
        }
        if y < item_top {
            return Ok(PlatformPointerAction::None);
        }
        let visible_index = usize::try_from((y - item_top) / item_height)
            .map_err(|_| Error::Abi("platform menu pointer index overflow".into()))?;
        let index = menu.first_visible_item.saturating_add(visible_index);
        Ok(menu
            .items
            .get(index)
            .and_then(Option::as_ref)
            .map(|_| PlatformPointerAction::MenuSelect(index))
            .unwrap_or(PlatformPointerAction::None))
    }

    pub(super) fn platform_dialog_pointer_action(
        &self,
        x: i32,
        y: i32,
    ) -> Result<PlatformPointerAction> {
        let (width, height) = self.screen_dimensions()?;
        if x < 0 || y < 0 || x >= width || y >= height {
            return Ok(PlatformPointerAction::None);
        }
        let softkey_top = height.saturating_sub(26);
        if y >= softkey_top {
            return Ok(if x < width / 2 {
                PlatformPointerAction::DialogAccept
            } else {
                PlatformPointerAction::DialogCancel
            });
        }
        let button_width = 120.min(width.saturating_sub(24));
        let button_x = (width - button_width) / 2;
        let button_y = height.saturating_sub(68);
        if x >= button_x && x < button_x + button_width && y >= button_y && y < button_y + 30 {
            return Ok(PlatformPointerAction::DialogAccept);
        }
        Ok(PlatformPointerAction::None)
    }

    pub(super) fn platform_text_viewer_pointer_action(
        &self,
        handle: u32,
        x: i32,
        y: i32,
    ) -> Result<PlatformPointerAction> {
        let Some(viewer) = self.text_viewers.get(&handle) else {
            return Ok(PlatformPointerAction::None);
        };
        let (width, height) = self.screen_dimensions()?;
        if x < 0 || y < 0 || x >= width || y >= height {
            return Ok(PlatformPointerAction::None);
        }
        let softkey_top = height.saturating_sub(26);
        Ok(if y < softkey_top {
            PlatformPointerAction::None
        } else if x >= width / 2 {
            PlatformPointerAction::TextViewerReturn
        } else if viewer.style == 1 {
            PlatformPointerAction::TextViewerAccept
        } else {
            PlatformPointerAction::None
        })
    }

    pub(super) fn render_platform_menu(
        &mut self,
        handle: u32,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        let (title, items, focused_item, mut first_visible_item) = {
            let menu = self
                .menus
                .get(&handle)
                .ok_or_else(|| Error::Abi(format!("missing platform menu handle {handle}")))?;
            (
                menu.title.clone(),
                menu.items.clone(),
                menu.focused_item,
                menu.first_visible_item,
            )
        };
        let (width, height) = self.screen_dimensions()?;
        let item_top = 34;
        let item_height = 24;
        let softkey_top = height.saturating_sub(26).max(item_top);
        let visible_items = usize::try_from((softkey_top - item_top) / item_height)
            .unwrap_or(0)
            .max(1);
        if focused_item < first_visible_item {
            first_visible_item = focused_item;
        } else if focused_item >= first_visible_item.saturating_add(visible_items) {
            first_visible_item = focused_item + 1 - visible_items;
        }

        let black = Framebuffer::rgb565(0, 0, 0);
        let green = Framebuffer::rgb565(0, 252, 0);
        let blue = Framebuffer::rgb565(0, 0, 248);
        self.draw_rectangle_to_screen(0, 0, width, height, black)?;
        self.draw_text_to_screen(&title, 8, 6, green, 0, services)?;
        self.draw_rectangle_to_screen(0, 26, width, 1, green)?;
        for (visible_index, item) in items
            .iter()
            .skip(first_visible_item)
            .take(visible_items)
            .enumerate()
        {
            let item_index = first_visible_item + visible_index;
            let y = item_top + i32::try_from(visible_index).unwrap_or(i32::MAX) * item_height;
            if item_index == focused_item {
                self.draw_rectangle_to_screen(0, y, width, item_height, blue)?;
            }
            if let Some(text) = item {
                self.draw_text_to_screen(text, 0, y + 4, green, 0, services)?;
            }
        }
        self.draw_rectangle_to_screen(0, softkey_top, width, 1, green)?;
        self.draw_text_to_screen(&[0x786e, 0x5b9a], 4, softkey_top + 5, green, 0, services)?;
        self.draw_text_to_screen(
            &[0x8fd4, 0x56de],
            width.saturating_sub(36),
            softkey_top + 5,
            green,
            0,
            services,
        )?;

        let menu_screen = self
            .memory
            .read(self.screen_base, self.platform_screen_byte_len()?)?;
        let menu = self
            .menus
            .get_mut(&handle)
            .expect("menu handle remained live while rendering");
        menu.first_visible_item = first_visible_item;
        menu.menu_screen = Some(menu_screen);
        self.present_screen(services)
    }

    pub(super) fn platform_screen_byte_len(&self) -> Result<usize> {
        let (width, height) = self.screen_dimensions()?;
        usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::Abi("platform UI screen size overflow".into()))
    }

    pub(super) fn capture_platform_screen(
        &self,
        services: &mut dyn NativeServices,
    ) -> Result<Vec<u8>> {
        let expected_len = self.platform_screen_byte_len()?;
        let Some(screen) = services.capture_framebuffer()? else {
            return self.memory.read(self.screen_base, expected_len);
        };
        if screen.len() != expected_len {
            return Err(Error::Abi(format!(
                "captured framebuffer is {} bytes, expected {expected_len}",
                screen.len()
            )));
        }
        Ok(screen)
    }

    pub(super) fn create_platform_dialog(
        &mut self,
        title: &[u16],
        message: &[u16],
        style: u32,
        services: &mut dyn NativeServices,
    ) -> Result<u32> {
        if style != 0 {
            return Err(Error::Abi(format!(
                "unsupported platform dialog style {style}"
            )));
        }
        let handle = self.allocate_ui_handle()?;
        let (width, height) = self.screen_dimensions()?;
        let screen_len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::Abi("platform dialog screen size overflow".into()))?;
        let selected_menu_background =
            self.pending_platform_menu_selection
                .take()
                .and_then(|handle| {
                    let ui = ActivePlatformUi::Menu(handle);
                    if self.active_platform_ui.last().copied() != Some(ui) {
                        return None;
                    }
                    self.active_platform_ui.pop();
                    if self
                        .platform_pointer_capture
                        .is_some_and(|capture| capture.ui == ui)
                    {
                        self.platform_pointer_capture = None;
                    }
                    let previous_screen = self
                        .menus
                        .get(&handle)
                        .and_then(|menu| menu.previous_screen.clone());
                    if previous_screen.is_some()
                        && let Some(menu) = self.menus.get_mut(&handle)
                    {
                        menu.modal_detached = true;
                    }
                    previous_screen
                });
        let previous_screen = match selected_menu_background {
            Some(screen) => screen,
            None => self.capture_platform_screen(services)?,
        };
        self.memory.write(self.screen_base, &previous_screen)?;

        let black = Framebuffer::rgb565(0, 0, 0);
        let green = Framebuffer::rgb565(0, 252, 0);
        let softkey_top = height.saturating_sub(26);
        self.draw_rectangle_to_screen(0, 0, width, height, black)?;
        self.draw_text_to_screen(title, 8, 6, green, 0, services)?;
        self.draw_rectangle_to_screen(0, 26, width, 1, green)?;
        self.draw_wrapped_text_to_screen(message, 12, 48, width - 24, green, services)?;
        self.draw_rectangle_to_screen(0, softkey_top, width, 1, green)?;
        self.draw_text_to_screen(&[0x786e, 0x5b9a], 4, softkey_top + 5, green, 0, services)?;
        self.draw_text_to_screen(
            &[0x8fd4, 0x56de],
            width.saturating_sub(36),
            softkey_top + 5,
            green,
            0,
            services,
        )?;

        let dialog_screen = self.memory.read(self.screen_base, screen_len)?;
        self.dialogs.insert(
            handle,
            PlatformDialog {
                previous_screen,
                dialog_screen,
            },
        );
        self.active_platform_ui
            .push(ActivePlatformUi::Dialog(handle));
        self.present_screen(services)?;
        Ok(handle)
    }

    pub(super) fn release_platform_dialog(
        &mut self,
        handle: u32,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        let Some(dialog) = self.dialogs.remove(&handle) else {
            return Ok(false);
        };
        let ui = ActivePlatformUi::Dialog(handle);
        let position = self
            .active_platform_ui
            .iter()
            .position(|active| *active == ui);
        let was_top =
            position.is_some_and(|position| position + 1 == self.active_platform_ui.len());
        if let Some(position) = position {
            self.active_platform_ui.remove(position);
        }
        if self
            .platform_pointer_capture
            .is_some_and(|capture| capture.ui == ui)
        {
            self.platform_pointer_capture = None;
        }
        if was_top {
            self.memory
                .write(self.screen_base, &dialog.previous_screen)?;
            self.present_screen(services)?;
        }
        Ok(true)
    }

    pub(super) fn create_platform_text_viewer(
        &mut self,
        title: &[u16],
        text: &[u16],
        style: u32,
        services: &mut dyn NativeServices,
    ) -> Result<u32> {
        if !matches!(style, 1 | 2) {
            return Err(Error::Abi(format!(
                "unsupported platform text-viewer style {style}"
            )));
        }
        let handle = self.allocate_ui_handle()?;
        let previous_screen = self
            .pending_platform_menu_selection
            .take()
            .and_then(|handle| {
                let ui = ActivePlatformUi::Menu(handle);
                if self.active_platform_ui.last().copied() != Some(ui) {
                    return None;
                }
                self.active_platform_ui.pop();
                if self
                    .platform_pointer_capture
                    .is_some_and(|capture| capture.ui == ui)
                {
                    self.platform_pointer_capture = None;
                }
                let previous_screen = self
                    .menus
                    .get(&handle)
                    .and_then(|menu| menu.previous_screen.clone());
                if previous_screen.is_some()
                    && let Some(menu) = self.menus.get_mut(&handle)
                {
                    menu.modal_detached = true;
                }
                previous_screen
            })
            .map(Ok)
            .unwrap_or_else(|| self.capture_platform_screen(services))?;
        self.memory.write(self.screen_base, &previous_screen)?;

        let (width, _) = self.screen_dimensions()?;
        self.text_viewers.insert(
            handle,
            PlatformTextViewer {
                previous_screen,
                style,
                title: title.to_vec(),
                lines: Self::wrap_platform_text(text, width.saturating_sub(16)),
                first_visible_line: 0,
                viewer_screen: Vec::new(),
            },
        );
        self.active_platform_ui
            .push(ActivePlatformUi::TextViewer(handle));
        if let Err(error) = self.render_platform_text_viewer(handle, services) {
            self.active_platform_ui.pop();
            if let Some(viewer) = self.text_viewers.remove(&handle) {
                self.memory
                    .write(self.screen_base, &viewer.previous_screen)?;
            }
            return Err(error);
        }
        Ok(handle)
    }

    pub(super) fn move_platform_text_viewer(
        &mut self,
        handle: u32,
        direction: i32,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        let (_, height) = self.screen_dimensions()?;
        let visible_lines = Self::platform_text_viewer_visible_lines(height);
        let Some(viewer) = self.text_viewers.get(&handle) else {
            return Ok(false);
        };
        let max_first_line = viewer.lines.len().saturating_sub(visible_lines);
        let next = if direction < 0 {
            viewer.first_visible_line.saturating_sub(1)
        } else {
            viewer
                .first_visible_line
                .saturating_add(1)
                .min(max_first_line)
        };
        if next == viewer.first_visible_line {
            return Ok(false);
        }
        self.text_viewers
            .get_mut(&handle)
            .expect("text viewer handle was checked")
            .first_visible_line = next;
        self.render_platform_text_viewer(handle, services)?;
        Ok(true)
    }

    fn render_platform_text_viewer(
        &mut self,
        handle: u32,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        let (style, title, lines, first_visible_line) = {
            let viewer = self.text_viewers.get(&handle).ok_or_else(|| {
                Error::Abi(format!("missing platform text viewer handle {handle}"))
            })?;
            (
                viewer.style,
                viewer.title.clone(),
                viewer.lines.clone(),
                viewer.first_visible_line,
            )
        };
        let (width, height) = self.screen_dimensions()?;
        let softkey_top = height.saturating_sub(PLATFORM_SOFTKEY_HEIGHT);
        let visible_lines = Self::platform_text_viewer_visible_lines(height);
        let max_first_line = lines.len().saturating_sub(visible_lines);
        let first_visible_line = first_visible_line.min(max_first_line);
        let black = Framebuffer::rgb565(0, 0, 0);
        let green = Framebuffer::rgb565(0, 252, 0);

        self.draw_rectangle_to_screen(0, 0, width, height, black)?;
        self.draw_text_to_screen(&title, 7, 6, green, 0, services)?;
        self.draw_rectangle_to_screen(0, 26, width, 1, green)?;
        for (visible_index, line) in lines
            .iter()
            .skip(first_visible_line)
            .take(visible_lines)
            .enumerate()
        {
            let y = TEXT_VIEWER_BODY_TOP
                + i32::try_from(visible_index).unwrap_or(i32::MAX) * TEXT_VIEWER_LINE_HEIGHT;
            self.draw_text_to_screen(line, 8, y, green, 0, services)?;
        }
        self.draw_platform_text_viewer_scrollbar(
            width,
            softkey_top,
            lines.len(),
            visible_lines,
            first_visible_line,
            green,
        )?;
        self.draw_rectangle_to_screen(0, softkey_top, width, 1, green)?;
        if style == 1 {
            self.draw_text_to_screen(&[0x786e, 0x5b9a], 4, softkey_top + 5, green, 0, services)?;
        }
        self.draw_text_to_screen(
            &[0x8fd4, 0x56de],
            width.saturating_sub(36),
            softkey_top + 5,
            green,
            0,
            services,
        )?;

        let viewer_screen = self
            .memory
            .read(self.screen_base, self.platform_screen_byte_len()?)?;
        let viewer = self
            .text_viewers
            .get_mut(&handle)
            .expect("text viewer handle remained live while rendering");
        viewer.first_visible_line = first_visible_line;
        viewer.viewer_screen = viewer_screen;
        self.present_screen(services)
    }

    fn draw_platform_text_viewer_scrollbar(
        &mut self,
        width: i32,
        softkey_top: i32,
        total_lines: usize,
        visible_lines: usize,
        first_visible_line: usize,
        color: u16,
    ) -> Result<()> {
        if total_lines <= visible_lines || visible_lines == 0 {
            return Ok(());
        }
        let track_height = softkey_top.saturating_sub(TEXT_VIEWER_BODY_TOP);
        if track_height <= 0 {
            return Ok(());
        }
        let thumb_height = (i64::from(track_height)
            * i64::try_from(visible_lines).unwrap_or(i64::MAX)
            / i64::try_from(total_lines).unwrap_or(i64::MAX))
        .clamp(12, i64::from(track_height)) as i32;
        let max_first_line = total_lines - visible_lines;
        let thumb_travel = track_height - thumb_height;
        let thumb_offset = i64::from(thumb_travel)
            * i64::try_from(first_visible_line).unwrap_or(i64::MAX)
            / i64::try_from(max_first_line).unwrap_or(i64::MAX);
        let track_x = width.saturating_sub(3);
        self.draw_rectangle_to_screen(track_x, TEXT_VIEWER_BODY_TOP, 1, track_height, color)?;
        self.draw_rectangle_to_screen(
            width.saturating_sub(6),
            TEXT_VIEWER_BODY_TOP + i32::try_from(thumb_offset).unwrap_or(i32::MAX),
            6,
            thumb_height,
            color,
        )
    }

    fn platform_text_viewer_visible_lines(height: i32) -> usize {
        let available_height = height
            .saturating_sub(PLATFORM_SOFTKEY_HEIGHT)
            .saturating_sub(TEXT_VIEWER_BODY_TOP);
        if available_height < TEXT_VIEWER_GLYPH_HEIGHT {
            return 0;
        }
        usize::try_from(1 + (available_height - TEXT_VIEWER_GLYPH_HEIGHT) / TEXT_VIEWER_LINE_HEIGHT)
            .unwrap_or(0)
    }

    fn wrap_platform_text(text: &[u16], max_width: i32) -> Vec<Vec<u16>> {
        let mut lines = Vec::new();
        let mut line = Vec::new();
        let mut line_width = 0_i32;
        for &codepoint in text {
            if codepoint == b'\n' as u16 {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
                continue;
            }
            let glyph_width = if codepoint < 128 { 8 } else { 16 };
            if !line.is_empty() && line_width.saturating_add(glyph_width) > max_width {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            }
            line.push(codepoint);
            line_width = line_width.saturating_add(glyph_width);
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }

    pub(super) fn release_platform_text_viewer(
        &mut self,
        handle: u32,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        let Some(viewer) = self.text_viewers.remove(&handle) else {
            return Ok(false);
        };
        let ui = ActivePlatformUi::TextViewer(handle);
        let position = self
            .active_platform_ui
            .iter()
            .position(|active| *active == ui);
        let was_top =
            position.is_some_and(|position| position + 1 == self.active_platform_ui.len());
        if let Some(position) = position {
            self.active_platform_ui.remove(position);
        }
        if self
            .platform_pointer_capture
            .is_some_and(|capture| capture.ui == ui)
        {
            self.platform_pointer_capture = None;
        }
        if was_top {
            self.memory
                .write(self.screen_base, &viewer.previous_screen)?;
            self.present_screen(services)?;
        }
        Ok(true)
    }

    pub(super) fn refresh_platform_text_viewer(
        &mut self,
        handle: u32,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        if self.active_platform_ui.last() != Some(&ActivePlatformUi::TextViewer(handle)) {
            return Ok(false);
        }
        let Some(viewer) = self.text_viewers.get(&handle) else {
            return Ok(false);
        };
        self.memory.write(self.screen_base, &viewer.viewer_screen)?;
        self.present_screen(services)?;
        Ok(true)
    }

    pub(super) fn draw_wrapped_text_to_screen(
        &mut self,
        text: &[u16],
        x: i32,
        mut y: i32,
        max_width: i32,
        color: u16,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        let mut line = Vec::new();
        let mut line_width = 0;
        for &codepoint in text {
            let glyph_width = if codepoint < 128 { 8 } else { 16 };
            if codepoint == b'\n' as u16
                || (!line.is_empty() && line_width + glyph_width > max_width)
            {
                self.draw_text_to_screen(&line, x, y, color, 0, services)?;
                line.clear();
                line_width = 0;
                y += 22;
                if codepoint == b'\n' as u16 {
                    continue;
                }
            }
            line.push(codepoint);
            line_width += glyph_width;
        }
        if !line.is_empty() {
            self.draw_text_to_screen(&line, x, y, color, 0, services)?;
        }
        Ok(())
    }

    pub(super) fn allocate_ui_handle(&mut self) -> Result<u32> {
        let live_handles = self
            .dialogs
            .len()
            .saturating_add(self.text_viewers.len())
            .saturating_add(self.editors.len())
            .saturating_add(self.menus.len())
            .saturating_add(self.native_windows.len());
        if live_handles >= MAX_PLATFORM_UI_HANDLES {
            return Err(Error::ResourceLimit(format!(
                "platform UI has {live_handles} live handles (limit {MAX_PLATFORM_UI_HANDLES})"
            )));
        }
        let start = self.next_ui_handle;
        loop {
            let handle = self.next_ui_handle;
            self.next_ui_handle = self.next_ui_handle.checked_add(1).unwrap_or(1);
            if handle != 0
                && !self.dialogs.contains_key(&handle)
                && !self.text_viewers.contains_key(&handle)
                && !self.editors.contains_key(&handle)
                && !self.menus.contains_key(&handle)
                && !self.native_windows.contains_key(&handle)
            {
                return Ok(handle);
            }
            if self.next_ui_handle == start {
                return Err(Error::ResourceLimit(
                    "no platform UI handles available".into(),
                ));
            }
        }
    }

    fn record_presented_screen_region(&mut self, x: i32, y: i32, width: usize, height: usize) {
        // A draw is authoritative parent output only after the top modal frame's
        // validated suspend witnesses have all finished.
        let returning_from_modal = self.modal_screens.last().is_some_and(|state| {
            state
                .active
                .iter()
                .all(|key| self.modal_screen_key_status(*key) != ModalScreenKeyStatus::Active)
        });
        if !returning_from_modal || self.presented_screen_pixels.is_none() {
            return;
        }
        let (screen_width, screen_height) = (
            i32::from(self.display_width),
            i32::from(self.display_height),
        );
        let presented = self
            .presented_screen_pixels
            .as_mut()
            .expect("presented pixel tracking was checked");
        if i32::from(presented.width) != screen_width
            || i32::from(presented.height) != screen_height
        {
            presented.compatible = false;
            return;
        }
        let screen_width = i64::from(screen_width);
        let screen_height = i64::from(screen_height);
        let width = i64::try_from(width).unwrap_or(i64::MAX);
        let height = i64::try_from(height).unwrap_or(i64::MAX);
        let x0 = i64::from(x).clamp(0, screen_width);
        let y0 = i64::from(y).clamp(0, screen_height);
        let x1 = i64::from(x).saturating_add(width).clamp(0, screen_width);
        let y1 = i64::from(y).saturating_add(height).clamp(0, screen_height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let stride = usize::from(presented.width);
        for row in y0 as usize..y1 as usize {
            let start = row * stride + x0 as usize;
            let end = row * stride + x1 as usize;
            presented.dirty[start..end].fill(true);
        }
    }

    pub(super) fn draw_platform_bitmap(
        &mut self,
        pixels: &[u8],
        x: i32,
        y: i32,
        width: usize,
        height: usize,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        services.draw_bitmap(pixels, x, y, width, height)?;
        self.record_presented_screen_region(x, y, width, height);
        Ok(())
    }

    pub(super) fn present_screen(&mut self, services: &mut dyn NativeServices) -> Result<()> {
        let (width, height) = self.screen_dimensions()?;
        let byte_len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::Abi("screen presentation size overflow".into()))?;
        let pixels = self.memory.read(self.screen_base, byte_len)?;
        self.draw_platform_bitmap(&pixels, 0, 0, width as usize, height as usize, services)
    }

    pub(super) fn read_platform_draw_pixels(
        &self,
        source: GuestAddr,
        x: i32,
        y: i32,
        width: usize,
        height: usize,
    ) -> Result<Vec<u8>> {
        let byte_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::Abi("mr_drawBitmap dimensions overflow".into()))?;
        if byte_len > self.heap_len {
            return Err(Error::Abi(format!(
                "mr_drawBitmap source is {byte_len} bytes"
            )));
        }
        if source != self.screen_base {
            return self.memory.read(source, byte_len);
        }

        let (screen_width, screen_height) = self.screen_dimensions()?;
        let region_width = i64::try_from(width)
            .map_err(|_| Error::Abi("mr_drawBitmap width exceeds i64".into()))?;
        let region_height = i64::try_from(height)
            .map_err(|_| Error::Abi("mr_drawBitmap height exceeds i64".into()))?;
        let region_end_x = i64::from(x) + region_width;
        let region_end_y = i64::from(y) + region_height;
        if x < 0
            || y < 0
            || region_end_x > i64::from(screen_width)
            || region_end_y > i64::from(screen_height)
        {
            return Err(Error::Abi(format!(
                "mr_drawBitmap screen region ({x}, {y}) {width}x{height} exceeds {screen_width}x{screen_height}"
            )));
        }

        let row_byte_len = width
            .checked_mul(2)
            .ok_or_else(|| Error::Abi("mr_drawBitmap row size overflow".into()))?;
        let mut pixels = Vec::with_capacity(byte_len);
        for row in 0..height {
            let row = i32::try_from(row)
                .map_err(|_| Error::Abi("mr_drawBitmap row exceeds i32".into()))?;
            let row_address = self.screen_address(x, y + row, screen_width)?;
            pixels.extend(self.memory.read(row_address, row_byte_len)?);
        }
        Ok(pixels)
    }

    pub(super) fn compact_ram_output_target(
        &mut self,
        package_address: GuestAddr,
        package_len: usize,
        output_len: usize,
        module: usize,
        output_len_pointer: GuestAddr,
    ) -> Result<Option<GuestAddr>> {
        if package_len < 24 {
            return Ok(None);
        }
        let header = self.memory.read(package_address, 24)?;
        if &header[..4] != b"MRPG"
            || read_le_u32(&header, 4)? != 4
            || read_le_u32(&header, 12)? != 4
        {
            return Ok(None);
        }

        let output_len = u32::try_from(output_len)
            .map_err(|_| Error::Abi("compact RAM MRP output length exceeds u32".into()))?;
        let aligned_len = heap::aligned_heap_len(output_len as usize)?;
        let wrapper_block_len = heap::aligned_heap_len(
            output_len
                .checked_add(4)
                .ok_or_else(|| Error::Abi("compact RAM wrapper length overflow".into()))?
                as usize,
        )?;
        let heap_end = HEAP_BASE.0 + self.heap_len as u32;
        let mut descriptor_matches = Vec::new();
        for descriptor_len_address in (HEAP_BASE.0 + 4..heap_end).step_by(4) {
            let recorded_len = self.memory.read_u32(GuestAddr(descriptor_len_address))?;
            if recorded_len != aligned_len {
                continue;
            }
            let candidate = self
                .memory
                .read_u32(GuestAddr(descriptor_len_address - 4))?;
            if candidate & 3 != 0 {
                continue;
            }
            let candidate = GuestAddr(candidate);
            if self
                .memory
                .check_range(candidate, aligned_len as usize, Permissions::READ_WRITE)
                .is_err()
            {
                continue;
            }
            if self.memory.read_u32(candidate.checked_add(4)?)? != aligned_len {
                continue;
            }
            let claimable = self.prepared_output_candidate_is_claimable_by_module(
                candidate,
                aligned_len,
                module,
            )?;
            if claimable {
                descriptor_matches.push((GuestAddr(descriptor_len_address - 4), candidate));
            }
        }
        let mut descriptor_candidates = descriptor_matches
            .iter()
            .map(|(_, candidate)| *candidate)
            .collect::<Vec<_>>();
        descriptor_candidates.sort_unstable();
        descriptor_candidates.dedup();
        // Some legacy readers keep `[prepared buffer, aligned length]` together
        // and pass the second word as mr_readFile's output-length pointer. A
        // returned page can leave an older pair intact, so prefer the pair tied
        // to this call before falling back to the heap-wide compatibility scan.
        if let Some(preferred_descriptor) = output_len_pointer.0.checked_sub(4).map(GuestAddr)
            && let Some(candidate) =
                descriptor_matches
                    .iter()
                    .find_map(|(descriptor, candidate)| {
                        (*descriptor == preferred_descriptor).then_some(*candidate)
                    })
        {
            return Ok(Some(candidate));
        }
        // This legacy reader keeps the prepared-buffer pointer immediately after
        // the output-length word in the current call's argument record.
        if output_len_pointer.0 != 0
            && let Some(candidate) = self.prepared_output_from_current_argument_record(
                output_len_pointer,
                &descriptor_candidates,
            )?
        {
            return Ok(Some(candidate));
        }
        match descriptor_candidates.as_slice() {
            [candidate] => return Ok(Some(*candidate)),
            [] => {}
            _ => {
                return Err(Error::Abi(format!(
                    "compact RAM MRP output has ambiguous prepared buffers: {descriptor_candidates:?}"
                )));
            }
        }

        // The legacy cfunction malloc wrapper asks the platform allocator for
        // `payload_len + 4`, stores payload_len at the backing address, and
        // returns backing + 4. Match that payload view directly. The backing
        // allocation remains the object freed by the corresponding wrapper. This
        // is only a fallback when the guest did not leave an explicit descriptor.
        let mut wrapper_candidates = Vec::new();
        for (&base, &block_len) in &self.guest_allocations {
            if block_len != wrapper_block_len
                || self.memory.read_u32(GuestAddr(base))? != output_len
            {
                continue;
            }
            let Some(candidate) = base.checked_add(4).map(GuestAddr) else {
                continue;
            };
            if self
                .memory
                .check_range(candidate, output_len as usize, Permissions::READ_WRITE)
                .is_err()
            {
                continue;
            }
            if self.prepared_output_candidate_is_claimable_by_module(
                candidate,
                aligned_len,
                module,
            )? {
                wrapper_candidates.push(candidate);
            }
        }
        wrapper_candidates.sort_unstable();
        wrapper_candidates.dedup();
        match wrapper_candidates.as_slice() {
            [] => Ok(None),
            [candidate] => Ok(Some(*candidate)),
            _ => Err(Error::Abi(format!(
                "compact RAM MRP output has ambiguous prepared buffers: {wrapper_candidates:?}"
            ))),
        }
    }

    fn prepared_output_from_current_argument_record(
        &self,
        output_len_pointer: GuestAddr,
        candidates: &[GuestAddr],
    ) -> Result<Option<GuestAddr>> {
        let Some(address) = output_len_pointer.0.checked_add(4).map(GuestAddr) else {
            return Ok(None);
        };
        if address.0 & 3 != 0
            || self
                .memory
                .check_range(address, 4, Permissions::READ)
                .is_err()
        {
            return Ok(None);
        }
        let value = GuestAddr(self.memory.read_u32(address)?);
        Ok(candidates.contains(&value).then_some(value))
    }

    pub(super) fn draw_bitmap_region_to_screen(
        &mut self,
        pixels: &[u8],
        x: i32,
        y: i32,
        width: usize,
        height: usize,
        mode: BitmapDrawMode,
    ) -> Result<()> {
        let (screen_width, screen_height) = self.screen_dimensions()?;
        let destination_x0 = i64::from(x).max(0);
        let destination_y0 = i64::from(y).max(0);
        let destination_x1 = (i64::from(x) + width as i64).min(i64::from(screen_width));
        let destination_y1 = (i64::from(y) + height as i64).min(i64::from(screen_height));
        if destination_x0 >= destination_x1 || destination_y0 >= destination_y1 {
            return Ok(());
        }

        let visible_width = usize::try_from(destination_x1 - destination_x0)
            .map_err(|_| Error::Abi("visible bitmap width exceeds usize".into()))?;
        let source_x = usize::try_from(destination_x0 - i64::from(x))
            .map_err(|_| Error::Abi("visible bitmap source x exceeds usize".into()))?;
        let source_y = usize::try_from(destination_y0 - i64::from(y))
            .map_err(|_| Error::Abi("visible bitmap source y exceeds usize".into()))?;
        let row_byte_len = visible_width
            .checked_mul(2)
            .ok_or_else(|| Error::Abi("visible bitmap row byte count overflow".into()))?;

        for visible_row in 0..usize::try_from(destination_y1 - destination_y0)
            .map_err(|_| Error::Abi("visible bitmap height exceeds usize".into()))?
        {
            let source_offset = (source_y + visible_row)
                .checked_mul(width)
                .and_then(|offset| offset.checked_add(source_x))
                .and_then(|offset| offset.checked_mul(2))
                .ok_or_else(|| Error::Abi("visible bitmap source offset overflow".into()))?;
            let source_row = &pixels[source_offset..source_offset + row_byte_len];
            let destination_address = self.active_screen_address(
                destination_x0 as i32,
                destination_y0 as i32 + visible_row as i32,
                screen_width,
            )?;
            if mode == BitmapDrawMode::Copy {
                self.memory.write(destination_address, source_row)?;
            } else {
                let mut destination_row = self.memory.read(destination_address, row_byte_len)?;
                for (source, destination) in source_row
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .zip(destination_row.as_chunks_mut::<2>().0.iter_mut())
                {
                    let source = u16::from_le_bytes(*source);
                    match mode {
                        BitmapDrawMode::Or => {
                            let destination_color =
                                u16::from_le_bytes([destination[0], destination[1]]);
                            destination
                                .copy_from_slice(&(source | destination_color).to_le_bytes());
                        }
                        BitmapDrawMode::Transparent(transparent_color)
                            if source != transparent_color =>
                        {
                            destination.copy_from_slice(&source.to_le_bytes());
                        }
                        BitmapDrawMode::Transparent(_) => {}
                        BitmapDrawMode::Gray(transparent_color) if source != transparent_color => {
                            destination.copy_from_slice(&grayscale_rgb565(source).to_le_bytes());
                        }
                        BitmapDrawMode::Gray(_) => {}
                        BitmapDrawMode::Copy => unreachable!("copy rows are handled above"),
                    }
                }
                self.memory.write(destination_address, &destination_row)?;
            }
        }
        Ok(())
    }

    pub(super) fn read_bitmap_descriptor(&self, address: GuestAddr) -> Result<BitmapDescriptor> {
        Ok(BitmapDescriptor {
            pixels: GuestAddr(self.memory.read_u32(address)?),
            width: usize::from(self.memory.read_u16(address.checked_add(4)?)?),
            height: usize::from(self.memory.read_u16(address.checked_add(6)?)?),
            x: i32::from(self.memory.read_u16(address.checked_add(8)?)? as i16),
            y: i32::from(self.memory.read_u16(address.checked_add(10)?)? as i16),
        })
    }

    pub(super) fn read_bitmap_transform(&self, address: GuestAddr) -> Result<BitmapTransform> {
        let read_field = |offset| {
            self.memory
                .read_u16(address.checked_add(offset)?)
                .map(|value| value as i16)
        };
        Ok(BitmapTransform {
            a: read_field(0)?,
            b: read_field(2)?,
            c: read_field(4)?,
            d: read_field(6)?,
            mode: read_field(8)?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn copy_transformed_bitmap(
        &mut self,
        destination: BitmapDescriptor,
        source: BitmapDescriptor,
        width: usize,
        height: usize,
        transform: BitmapTransform,
        transparent_color: u16,
        module: usize,
    ) -> Result<()> {
        let transparent_color = match transform.mode {
            2 => None,
            6 => Some(transparent_color),
            mode => {
                return Err(Error::Abi(format!(
                    "unsupported transformed bitmap mode {mode} called by module {module}"
                )));
            }
        };
        if width == 0 || height == 0 {
            return Ok(());
        }

        let source_x = usize::try_from(source.x).map_err(|_| {
            Error::Abi(format!("negative transformed bitmap source x {}", source.x))
        })?;
        let source_y = usize::try_from(source.y).map_err(|_| {
            Error::Abi(format!("negative transformed bitmap source y {}", source.y))
        })?;
        let source_end_x = source_x
            .checked_add(width)
            .ok_or_else(|| Error::Abi("transformed bitmap source width overflow".into()))?;
        let source_end_y = source_y
            .checked_add(height)
            .ok_or_else(|| Error::Abi("transformed bitmap source height overflow".into()))?;
        if source_end_x > source.width || source_end_y > source.height {
            return Err(Error::Abi(format!(
                "transformed bitmap source region ({source_x}, {source_y}) {width}x{height} exceeds {}x{} bitmap",
                source.width, source.height
            )));
        }
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| Error::Abi("transformed bitmap region dimensions overflow".into()))?;
        if pixel_count > self.heap_len / 2 {
            return Err(Error::Abi(format!(
                "transformed bitmap region requires {pixel_count} pixels"
            )));
        }

        // Source and destination can refer to the same bitmap. Capture the
        // complete source region before changing any destination pixel.
        let mut pixels = Vec::with_capacity(pixel_count);
        for row in 0..height {
            for column in 0..width {
                let address = bitmap_pixel_address(
                    source.pixels,
                    source.width,
                    source_x + column,
                    source_y + row,
                )?;
                pixels.push(self.memory.read_u16(address)?);
            }
        }

        let last_x = i64::try_from(width - 1)
            .map_err(|_| Error::Abi("transformed bitmap width exceeds i64".into()))?;
        let last_y = i64::try_from(height - 1)
            .map_err(|_| Error::Abi("transformed bitmap height exceeds i64".into()))?;
        let corners = [
            transform.apply(0, 0),
            transform.apply(last_x, 0),
            transform.apply(0, last_y),
            transform.apply(last_x, last_y),
        ];
        let minimum_x = corners
            .iter()
            .map(|(x, _)| *x)
            .min()
            .expect("four transform corners");
        let minimum_y = corners
            .iter()
            .map(|(_, y)| *y)
            .min()
            .expect("four transform corners");

        for row in 0..height {
            for column in 0..width {
                let color = pixels[row * width + column];
                if Some(color) == transparent_color {
                    continue;
                }
                let (transformed_x, transformed_y) = transform.apply(column as i64, row as i64);
                let destination_x = i64::from(destination.x) + transformed_x - minimum_x;
                let destination_y = i64::from(destination.y) + transformed_y - minimum_y;
                if destination_x < 0
                    || destination_y < 0
                    || destination_x >= destination.width as i64
                    || destination_y >= destination.height as i64
                {
                    continue;
                }
                let address = bitmap_pixel_address(
                    destination.pixels,
                    destination.width,
                    destination_x as usize,
                    destination_y as usize,
                )?;
                self.memory.write_u16(address, color)?;
            }
        }
        Ok(())
    }

    pub(super) fn draw_rectangle_to_screen(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        color: u16,
    ) -> Result<()> {
        if width <= 0 || height <= 0 {
            return Ok(());
        }
        let (screen_width, screen_height) = self.screen_dimensions()?;
        let x0 = x.clamp(0, screen_width);
        let y0 = y.clamp(0, screen_height);
        let x1 = x.saturating_add(width).clamp(0, screen_width);
        let y1 = y.saturating_add(height).clamp(0, screen_height);
        if x0 >= x1 || y0 >= y1 {
            return Ok(());
        }
        let color = color.to_le_bytes();
        let mut row = Vec::with_capacity((x1 - x0) as usize * 2);
        for _ in x0..x1 {
            row.extend_from_slice(&color);
        }
        for screen_y in y0..y1 {
            let address = self.screen_address(x0, screen_y, screen_width)?;
            self.memory.write(address, &row)?;
        }
        Ok(())
    }

    pub(super) fn draw_text_to_screen(
        &mut self,
        text: &[u16],
        mut x: i32,
        y: i32,
        color: u16,
        font: u32,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        let (screen_width, screen_height) = self.screen_dimensions()?;
        for &codepoint in text {
            let Some((glyph, width, height)) = services.char_bitmap(u32::from(codepoint), font)?
            else {
                x += if codepoint < 128 { 8 } else { 16 };
                continue;
            };
            let width = width.min(16) as i32;
            let height = height.min(16) as usize;
            let required = height
                .checked_mul(2)
                .ok_or_else(|| Error::Abi("character bitmap size overflow".into()))?;
            if glyph.len() < required {
                return Err(Error::Abi(format!(
                    "character bitmap for {codepoint:#06x} has {} bytes, needs {required}",
                    glyph.len()
                )));
            }
            for row in 0..height as i32 {
                let offset = row as usize * 2;
                let bits = u16::from_be_bytes([glyph[offset], glyph[offset + 1]]);
                for column in 0..width {
                    if bits & (0x8000_u16 >> column) != 0 {
                        self.write_screen_pixel(
                            x + column,
                            y + row,
                            color,
                            screen_width,
                            screen_height,
                        )?;
                    }
                }
            }
            x += width;
        }
        Ok(())
    }

    pub(super) fn write_screen_pixel(
        &mut self,
        x: i32,
        y: i32,
        color: u16,
        width: i32,
        height: i32,
    ) -> Result<()> {
        if x < 0 || y < 0 || x >= width || y >= height {
            return Ok(());
        }
        let address = self.screen_address(x, y, width)?;
        self.memory.write_u16(address, color)
    }

    pub(super) fn screen_dimensions(&self) -> Result<(i32, i32)> {
        let width = self.memory.read_u32(data_slot_address(92))?;
        let height = self.memory.read_u32(data_slot_address(93))?;
        Ok((
            i32::try_from(width)
                .map_err(|_| Error::Abi(format!("screen width {width} exceeds i32")))?,
            i32::try_from(height)
                .map_err(|_| Error::Abi(format!("screen height {height} exceeds i32")))?,
        ))
    }

    pub(super) fn set_screen_orientation(
        &mut self,
        landscape: bool,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        let (width, height) = (self.display_width, self.display_height);
        let short = width.min(height);
        let long = width.max(height);
        let (width, height) = if landscape {
            (long, short)
        } else {
            (short, long)
        };
        let resize_display = (self.display_width, self.display_height) != (width, height);

        let byte_len = usize::from(width)
            .checked_mul(usize::from(height))
            .and_then(|pixels| pixels.checked_mul(2))
            .and_then(|len| u32::try_from(len).ok())
            .ok_or_else(|| Error::Abi("screen bitmap size overflow".into()))?;
        self.memory
            .check_range(self.screen_base, byte_len as usize, Permissions::READ_WRITE)?;
        let bitmap_table = GuestAddr(self.memory.read_u32(table_slot_address(95))?);
        let screen_bitmap = bitmap_table.checked_add(SCREEN_BITMAP_ID * BITMAP_ENTRY_SIZE)?;
        self.memory
            .check_range(screen_bitmap, 8, Permissions::READ_WRITE)?;

        if resize_display {
            services.resize_screen(width, height)?;
            self.display_width = width;
            self.display_height = height;
        }
        self.memory
            .write_u32(data_slot_address(92), u32::from(width))?;
        self.memory
            .write_u32(data_slot_address(93), u32::from(height))?;
        self.memory.write_u16(screen_bitmap, width)?;
        self.memory
            .write_u16(screen_bitmap.checked_add(2)?, height)?;
        self.memory
            .write_u32(screen_bitmap.checked_add(4)?, byte_len)?;
        if resize_display {
            self.present_screen(services)?;
        }
        Ok(())
    }

    pub(super) fn screen_address(&self, x: i32, y: i32, width: i32) -> Result<GuestAddr> {
        screen_pixel_address(self.screen_base, x, y, width)
    }

    fn active_screen_address(&self, x: i32, y: i32, width: i32) -> Result<GuestAddr> {
        let screen = GuestAddr(self.memory.read_u32(data_slot_address(91))?);
        screen_pixel_address(screen, x, y, width)
    }
}

fn screen_pixel_address(screen: GuestAddr, x: i32, y: i32, width: i32) -> Result<GuestAddr> {
    let offset = y
        .checked_mul(width)
        .and_then(|offset| offset.checked_add(x))
        .and_then(|offset| offset.checked_mul(2))
        .and_then(|offset| u32::try_from(offset).ok())
        .ok_or_else(|| Error::Abi("screen pixel offset overflow".into()))?;
    screen.checked_add(offset)
}

fn grayscale_rgb565(color: u16) -> u16 {
    let red = u32::from((color >> 11) & 0x1f);
    let green = u32::from((color >> 5) & 0x3f);
    let blue = u32::from(color & 0x1f);
    let red = (red << 3) | (red >> 2);
    let green = (green << 2) | (green >> 4);
    let blue = (blue << 3) | (blue >> 2);
    // The caller only defines gray-with-transparent-key semantics. Use a
    // deterministic integer BT.601 conversion for the platform implementation.
    let luminance = ((77 * red + 150 * green + 29 * blue + 128) >> 8) as i32;
    Framebuffer::rgb565(luminance, luminance, luminance)
}

impl BitmapTransform {
    fn apply(self, x: i64, y: i64) -> (i64, i64) {
        (
            (i64::from(self.a) * x + i64::from(self.b) * y) >> 8,
            (i64::from(self.c) * x + i64::from(self.d) * y) >> 8,
        )
    }
}

fn bitmap_pixel_address(pixels: GuestAddr, stride: usize, x: usize, y: usize) -> Result<GuestAddr> {
    let byte_offset = y
        .checked_mul(stride)
        .and_then(|offset| offset.checked_add(x))
        .and_then(|offset| offset.checked_mul(2))
        .and_then(|offset| u32::try_from(offset).ok())
        .ok_or_else(|| Error::Abi("bitmap pixel offset overflow".into()))?;
    pixels.checked_add(byte_offset)
}
