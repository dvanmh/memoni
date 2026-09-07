use std::cell::RefCell;

use anyhow::Result;
use log::debug;
use x11rb::{
    connection::Connection as _,
    protocol::xproto::{ConnectionExt as _, GetKeyboardMappingReply},
    xcb_ffi::XCBConnection,
};
use xkeysym::{KeyCode, Keysym, keysym as xkeysym_keycode_to_keysym};

pub struct X11KeyConverter<'a> {
    conn: &'a XCBConnection,
    min_keycode: RefCell<u8>,
    max_keycode: RefCell<u8>,
    mapping: RefCell<GetKeyboardMappingReply>,
}

impl<'a> X11KeyConverter<'a> {
    pub fn new(conn: &'a XCBConnection) -> Result<Self> {
        let setup = conn.setup();
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let mapping_reply = conn
            .get_keyboard_mapping(min_keycode, max_keycode - min_keycode + 1)?
            .reply()?;

        Ok(Self {
            conn,
            min_keycode: RefCell::new(min_keycode),
            max_keycode: RefCell::new(max_keycode),
            mapping: RefCell::new(mapping_reply),
        })
    }

    pub fn update_mapping(&self) -> Result<()> {
        let setup = self.conn.setup();
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let mapping_reply = self
            .conn
            .get_keyboard_mapping(min_keycode, max_keycode - min_keycode + 1)?
            .reply()?;

        let mut self_min_keycode = self.min_keycode.borrow_mut();
        let mut self_max_keycode = self.max_keycode.borrow_mut();
        let mut self_mapping = self.mapping.borrow_mut();
        if *self_min_keycode != min_keycode
            || *self_max_keycode != max_keycode
            || self_mapping.keysyms_per_keycode != mapping_reply.keysyms_per_keycode
            || self_mapping.keysyms != mapping_reply.keysyms
        {
            debug!("keyboard mapping changed; using new mapping");
            *self_min_keycode = min_keycode;
            *self_max_keycode = max_keycode;
            *self_mapping = mapping_reply;
        }

        Ok(())
    }

    pub fn keycode_to_keysym(&self, keycode: KeyCode) -> Option<Keysym> {
        self.keycode_to_keysym_column(keycode, 0)
    }

    pub fn keycode_to_keysym_column(&self, keycode: KeyCode, column: u8) -> Option<Keysym> {
        let min_keycode = *self.min_keycode.borrow();
        let mapping = self.mapping.borrow();

        xkeysym_keycode_to_keysym(
            keycode,
            column,
            min_keycode.into(),
            mapping.keysyms_per_keycode,
            &mapping.keysyms,
        )
    }

    pub fn keysym_to_keycode(&self, keysym: Keysym) -> Option<KeyCode> {
        let min_keycode = *self.min_keycode.borrow();
        let mapping = self.mapping.borrow();

        for (i, keysyms) in mapping
            .keysyms
            .chunks(mapping.keysyms_per_keycode as usize)
            .enumerate()
        {
            for &ks in keysyms {
                if ks == keysym.into() {
                    let keycode = min_keycode + i as u8;
                    return Some(keycode.into());
                }
            }
        }

        None
    }
}
