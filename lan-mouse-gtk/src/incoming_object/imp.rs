use std::cell::RefCell;

use glib::{ParamSpec, ParamSpecDouble, ParamSpecString, Value};
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::IncomingData;

#[derive(Default)]
pub struct IncomingObject {
    pub data: RefCell<IncomingData>,
}

#[glib::object_subclass]
impl ObjectSubclass for IncomingObject {
    const NAME: &'static str = "IncomingObject";
    type Type = super::IncomingObject;
}

impl ObjectImpl for IncomingObject {
    fn properties() -> &'static [ParamSpec] {
        use std::sync::OnceLock;
        static PROPERTIES: OnceLock<Vec<ParamSpec>> = OnceLock::new();
        PROPERTIES.get_or_init(|| {
            vec![
                ParamSpecString::builder("fingerprint").build(),
                ParamSpecString::builder("description").build(),
                ParamSpecString::builder("address").build(),
                ParamSpecString::builder("position").build(),
                ParamSpecDouble::builder("pointer-scale")
                    .minimum(0.0)
                    .maximum(1.0)
                    .default_value(0.5)
                    .build(),
            ]
        })
    }

    fn property(&self, _id: usize, pspec: &ParamSpec) -> Value {
        let data = self.data.borrow();
        match pspec.name() {
            "fingerprint" => data.fingerprint.to_value(),
            "description" => data.description.to_value(),
            "address" => data.address.to_value(),
            "position" => data.position.to_value(),
            "pointer-scale" => data.pointer_scale.to_value(),
            _ => unimplemented!(),
        }
    }

    fn set_property(&self, _id: usize, value: &Value, pspec: &ParamSpec) {
        let mut data = self.data.borrow_mut();
        match pspec.name() {
            "fingerprint" => data.fingerprint = value.get().unwrap(),
            "description" => data.description = value.get().unwrap(),
            "address" => data.address = value.get().unwrap(),
            "position" => data.position = value.get().unwrap(),
            "pointer-scale" => data.pointer_scale = value.get().unwrap(),
            _ => unimplemented!(),
        }
    }
}
