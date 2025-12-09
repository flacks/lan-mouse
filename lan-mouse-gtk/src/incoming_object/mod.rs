mod imp;

use adw::subclass::prelude::*;
use gtk::glib::{self, Object};
use gtk::prelude::*;
use std::net::SocketAddr;
use lan_mouse_ipc::Position;

glib::wrapper! {
    pub struct IncomingObject(ObjectSubclass<imp::IncomingObject>);
}

impl IncomingObject {
    pub fn new(fingerprint: String, description: String, addr: SocketAddr, pos: Position, pointer_scale: f64) -> Self {
        Object::builder()
            .property("fingerprint", fingerprint)
            .property("description", description)
            .property("address", addr.to_string())
            .property("position", pos.to_string())
            .property("pointer-scale", pointer_scale)
            .build()
    }

    pub fn get_data(&self) -> IncomingData {
        self.imp().data.borrow().clone()
    }

    pub fn fingerprint(&self) -> String {
        self.property("fingerprint")
    }

    pub fn set_fingerprint(&self, fingerprint: String) {
        self.set_property("fingerprint", fingerprint);
    }

    pub fn description(&self) -> String {
        self.property("description")
    }

    pub fn set_description(&self, description: String) {
        self.set_property("description", description);
    }

    pub fn address(&self) -> String {
        self.property("address")
    }

    pub fn set_address(&self, address: String) {
        self.set_property("address", address);
    }

    pub fn position(&self) -> String {
        self.property("position")
    }

    pub fn set_position(&self, position: String) {
        self.set_property("position", position);
    }

    pub fn pointer_scale(&self) -> f64 {
        self.property("pointer-scale")
    }

    pub fn set_pointer_scale(&self, pointer_scale: f64) {
        self.set_property("pointer-scale", pointer_scale);
    }
}

#[derive(Clone)]
pub struct IncomingData {
    pub fingerprint: String,
    pub description: String,
    pub address: String,
    pub position: String,
    pub pointer_scale: f64,
}

impl Default for IncomingData {
    fn default() -> Self {
        Self {
            fingerprint: String::default(),
            description: String::default(),
            address: String::default(),
            position: String::default(),
            pointer_scale: 0.5, // Must be within 0.0-1.0 range
        }
    }
}
