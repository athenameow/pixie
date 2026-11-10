//! Fills the monitor-sized surface and places the card at a fixed size:
//! centred horizontally, with its top edge a fixed fraction down the monitor.
//! Placement happens in the normal layout pass, so the first frame already has
//! the card in place, and two stages with the same size place their cards on
//! exactly the same pixels (the glass and the content are separate surfaces).

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// The card's top edge sits this far down the monitor.
pub const TOP: f64 = 0.2;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Stage {
        pub size: std::cell::Cell<(i32, i32)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Stage {
        const NAME: &'static str = "PixieStage";
        type Type = super::Stage;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Stage {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Stage {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            match self.obj().first_child() {
                Some(child) => {
                    let (min, nat, _, _) = child.measure(orientation, for_size);
                    (min, nat, -1, -1)
                }
                None => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some(child) = self.obj().first_child() else {
                return;
            };
            let (child_width, child_height) = self.size.get();
            let x = (width - child_width) / 2;
            let y = ((height as f64 * TOP).round() as i32)
                .min(height - child_height)
                .max(0);
            child.size_allocate(&gtk::Allocation::new(x, y, child_width, child_height), -1);
        }
    }
}

glib::wrapper! {
    pub struct Stage(ObjectSubclass<imp::Stage>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Stage {
    pub fn new(child: &impl IsA<gtk::Widget>, size: (i32, i32)) -> Self {
        let stage: Self = glib::Object::new();
        stage.imp().size.set(size);
        child.set_size_request(size.0, size.1);
        child.set_parent(&stage);
        stage
    }
}
