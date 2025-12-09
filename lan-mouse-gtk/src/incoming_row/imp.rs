use std::cell::RefCell;

use adw::subclass::prelude::*;
use adw::{ActionRow, prelude::*};
use glib::{Binding, subclass::InitializingObject};
use gtk::glib::subclass::Signal;
use gtk::glib::{SignalHandlerId, clone};
use gtk::{CompositeTemplate, glib};
use std::sync::OnceLock;

use crate::incoming_object::IncomingObject;

#[derive(CompositeTemplate, Default)]
#[template(resource = "/de/feschber/LanMouse/incoming_row.ui")]
pub struct IncomingRow {
    #[template_child]
    pub pointer_scale_row: TemplateChild<ActionRow>,
    #[template_child]
    pub pointer_scale: TemplateChild<gtk::Scale>,
    pub bindings: RefCell<Vec<Binding>>,
    pointer_scale_change_handler: RefCell<Option<SignalHandlerId>>,
    pub incoming_object: RefCell<Option<IncomingObject>>,
}

#[glib::object_subclass]
impl ObjectSubclass for IncomingRow {
    const NAME: &'static str = "IncomingRow";
    const ABSTRACT: bool = false;

    type Type = super::IncomingRow;
    type ParentType = adw::ExpanderRow;

    fn class_init(klass: &mut Self::Class) {
        klass.bind_template();
        klass.bind_template_callbacks();
    }

    fn instance_init(obj: &InitializingObject<Self>) {
        obj.init_template();
    }
}

impl ObjectImpl for IncomingRow {
    fn constructed(&self) {
        self.parent_constructed();
        let handler = self.pointer_scale.connect_value_changed(clone!(
            #[weak(rename_to = row)]
            self,
            move |scale| {
                row.handle_pointer_scale_changed(scale);
            }
        ));
        self.pointer_scale_change_handler.replace(Some(handler));
    }

    fn signals() -> &'static [glib::subclass::Signal] {
        static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
        SIGNALS.get_or_init(|| {
            vec![
                Signal::builder("request-pointer-scale-change")
                    .param_types([f64::static_type()])
                    .build(),
            ]
        })
    }
}

#[gtk::template_callbacks]
impl IncomingRow {
    fn handle_pointer_scale_changed(&self, scale: &gtk::Scale) {
        let value = scale.value();
        log::info!("Incoming connection pointer scale slider changed to: {:.2}", value);
        self.obj()
            .emit_by_name::<()>("request-pointer-scale-change", &[&value]);
    }

    pub(super) fn set_pointer_scale(&self, scale: f64) {
        let handler = self.pointer_scale_change_handler.borrow();
        let handler = handler.as_ref().expect("signal handler");
        self.pointer_scale.block_signal(handler);
        let scale_adjustment = self.pointer_scale.adjustment();
        scale_adjustment.set_value(scale);
        self.pointer_scale.unblock_signal(handler);
    }
}

impl WidgetImpl for IncomingRow {}
impl BoxImpl for IncomingRow {}
impl ListBoxRowImpl for IncomingRow {}
impl PreferencesRowImpl for IncomingRow {}
impl ExpanderRowImpl for IncomingRow {}
