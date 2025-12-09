mod imp;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::Object;
use gtk::glib;

use crate::incoming_object::IncomingObject;

glib::wrapper! {
    pub struct IncomingRow(ObjectSubclass<imp::IncomingRow>)
        @extends adw::ExpanderRow, adw::PreferencesRow, gtk::ListBoxRow, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for IncomingRow {
    fn default() -> Self {
        Self::new()
    }
}

impl IncomingRow {
    pub fn new() -> Self {
        Object::builder().build()
    }

    pub fn bind(&self, incoming_object: &IncomingObject) {
        let imp = self.imp();

        // Show description as title if available, otherwise show fingerprint
        let description = incoming_object.description();
        let title = if !description.is_empty() {
            description
        } else {
            incoming_object.fingerprint()
        };
        self.set_title(&title);

        // Show "fingerprint @ address" as subtitle
        let subtitle = format!("{} @ {}", 
            incoming_object.fingerprint(), 
            incoming_object.address()
        );
        self.set_subtitle(&subtitle);

        let description_binding = incoming_object
            .bind_property("description", self, "title")
            .transform_to(|_, desc: String| {
                if desc.is_empty() {
                    None  // Will keep using fingerprint
                } else {
                    Some(desc)
                }
            })
            .sync_create()
            .build();

        let mut bindings = imp.bindings.borrow_mut();
        bindings.push(description_binding);

        // Set initial pointer scale value
        imp.set_pointer_scale(incoming_object.pointer_scale());

        // Store the incoming_object
        imp.incoming_object.replace(Some(incoming_object.clone()));
    }

    pub fn unbind(&self) {
        let imp = self.imp();
        for binding in imp.bindings.borrow_mut().drain(..) {
            binding.unbind();
        }
        imp.incoming_object.replace(None);
    }

    pub fn incoming_object(&self) -> Option<IncomingObject> {
        self.imp().incoming_object.borrow().clone()
    }
}
