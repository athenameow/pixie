//! pixie: an app launcher popup. Runs only while open; launching it again
//! while it is open closes it, so one keybind toggles it.

mod apps;
mod calc;
mod history;
mod launch;
mod rank;
mod stage;

use apps::App;
use gtk::glib::BoxedAnyObject;
use gtk::{gdk, gio, glib, pango, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use history::History;
use launch::Launch;
use rank::Ranker;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::OnceLock;
use std::time::Instant;

const APP_ID: &str = "io.athena.pixie";
const WIDTH: i32 = 560;
const VISIBLE_ROWS: i32 = 8;
/// Row height including the gap below it; must match the style sheet
/// (32 px content + 2 × 8 px padding + 2 px margin).
const ROW_PITCH: i32 = 50;
/// Card size, fixed so the glass and the content (separate surfaces) line up:
/// 40 px search + 6 px gap + the rows, plus 2 × 6 px padding and 2 × 1 px border.
const CARD_SIZE: (i32, i32) = (WIDTH, 40 + 6 + VISIBLE_ROWS * ROW_PITCH + 14);
/// Most rows shown at once. Ranking 300 apps takes ~0.05 ms, but swapping
/// several hundred rows into the list costs ~8 ms per keystroke; nobody scrolls
/// that far in a launcher.
const MAX_RESULTS: usize = 40;
const ICON_SIZE: i32 = 32;
const ACTION_ICON_SIZE: i32 = 24;

/// One row in the list.
enum Item {
    App(usize),
    Action {
        app: usize,
        id: String,
        name: String,
    },
    Calc {
        expr: String,
        value: String,
    },
    Command(String),
}

struct Ui {
    window: gtk::ApplicationWindow,
    search: gtk::SearchEntry,
    status: gtk::Label,
    model: gio::ListStore,
    selection: gtk::SingleSelection,
    list: gtk::ListView,
    list_stack: gtk::Stack,
    empty: gtk::Label,
    apps: Vec<App>,
    history: RefCell<History>,
    ranker: RefCell<Ranker>,
    /// The app whose desktop actions are shown below it.
    expanded: Cell<Option<usize>>,
    /// Last pointer position; hovering selects only once the pointer really
    /// moves, not when rows change under a still pointer.
    pointer: Cell<Option<(f64, f64)>>,
    pointer_moved: Cell<bool>,
    /// Rows held back from the first frame (see `refresh`).
    deferred: RefCell<Vec<BoxedAnyObject>>,
}

static START: OnceLock<Instant> = OnceLock::new();

/// With `PIXIE_TIMING` set, prints milliseconds since `main` and the wall
/// clock (to compare with the time a key was pressed).
fn mark(label: &str) {
    static ON: OnceLock<bool> = OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("PIXIE_TIMING").is_some()) {
        let start = START.get_or_init(Instant::now);
        let wall = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        eprintln!(
            "pixie: {label:<18} {:7.2} ms  (wall {}.{:06})",
            start.elapsed().as_secs_f64() * 1000.0,
            wall.as_secs(),
            wall.subsec_micros()
        );
    }
}

fn main() -> glib::ExitCode {
    START.get_or_init(Instant::now);
    mark("main");
    // Startup tuning, each overridable by setting the variable yourself:
    // - Software rendering opens faster and uses far less memory than GL/Vulkan
    //   for a popup this small.
    // - Skipping xdg-desktop-portal saves ~170 ms of GTK startup; the popup uses
    //   no portal features.
    // Launched apps don't inherit these (see launch::OWN_VARS).
    let mut own = Vec::new();
    for (var, value) in [("GSK_RENDERER", "cairo"), ("GDK_DEBUG", "no-portals")] {
        if std::env::var_os(var).is_none() {
            // SAFETY: no other threads exist yet.
            unsafe { std::env::set_var(var, value) };
            own.push(var);
        }
    }
    // The `pixie` launcher took this out to skip loading the GL vendor library
    // (see shim.rs); launched apps get it back.
    if let Some(value) = std::env::var_os("PIXIE_SAVED_GLX_VENDOR") {
        // SAFETY: no other threads exist yet.
        unsafe {
            std::env::set_var("__GLX_VENDOR_LIBRARY_NAME", value);
            std::env::remove_var("PIXIE_SAVED_GLX_VENDOR");
        }
    }
    launch::OWN_VARS.set(own).unwrap();
    // Launched apps start where pixie is; make that home rather than wherever
    // the compositor or a terminal happened to be.
    let _ = std::env::set_current_dir(glib::home_dir());

    // Read the app index and launch history while GTK starts up.
    let loader = Cell::new(Some(std::thread::spawn(|| {
        let index = (apps::load(), History::load());
        mark("apps loaded");
        index
    })));

    let app = gtk::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(move |app| match app.windows().first() {
        Some(_) => app.windows().iter().for_each(|w| w.close()),
        None => {
            if let Some(loader) = loader.take() {
                let (apps, history) = loader.join().unwrap_or_default();
                build(app, apps, history);
            }
        }
    });
    // Arguments are ignored: every launch is a toggle.
    app.run_with_args::<&str>(&[])
}

fn load_css() {
    mark("gtk ready");
    let display = gdk::Display::default().expect("no display");
    // Portals are skipped, so GTK can't learn the desktop's dark preference; the
    // popup is always dark.
    gtk::Settings::for_display(&display)
        .set_gtk_interface_color_scheme(gtk::InterfaceColorScheme::Dark);
    let builtin = gtk::CssProvider::new();
    builtin.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &builtin,
        gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
    );

    let custom = glib::user_config_dir().join("pixie").join("style.css");
    if custom.exists() {
        let provider = gtk::CssProvider::new();
        provider.load_from_path(&custom);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 2,
        );
    }
}

/// A transparent layer surface covering the monitor. With no output set, the
/// compositor puts it on the focused monitor.
fn layer_window(app: &gtk::Application, keyboard: KeyboardMode) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("pixie")
        .build();
    window.add_css_class("pixie");
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("pixie"));
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.set_keyboard_mode(keyboard);
    window
}

fn build(app: &gtk::Application, apps: Vec<App>, history: History) {
    // Two surfaces. The glass (frosted card, blurred by the compositor) sits on
    // its own surface and is drawn once. GTK's software renderer redraws a
    // backdrop-filter's blur pass whenever anything above it changes, about
    // 5 ms per keystroke for a card this size, even though the compositor does
    // the actual blurring.
    let glass = layer_window(app, KeyboardMode::None);
    let glass_card = gtk::Box::builder().css_classes(["card"]).build();
    glass.set_child(Some(&stage::Stage::new(&glass_card, CARD_SIZE)));
    glass.set_can_target(false);

    // The content, above the glass: like a shell panel, the surface covers the
    // monitor (transparent outside the card) so a click outside closes it, and
    // it holds the keyboard while open.
    let window = layer_window(app, KeyboardMode::Exclusive);

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search apps")
        .hexpand(true)
        .build();
    let status = gtk::Label::builder()
        .css_classes(["status"])
        .visible(false)
        .build();
    let header = hbox(6, &[search.upcast_ref(), status.upcast_ref()]);
    header.add_css_class("header");

    let model = gio::ListStore::new::<BoxedAnyObject>();
    let selection = gtk::SingleSelection::new(Some(model.clone()));
    let list = gtk::ListView::builder()
        .model(&selection)
        .css_classes(["results"])
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .child(&list)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let empty = gtk::Label::builder().css_classes(["dim", "empty"]).build();
    let list_stack = gtk::Stack::new();
    list_stack.add_named(&scroller, Some("list"));
    list_stack.add_named(&empty, Some("empty"));
    list_stack.set_vexpand(true);

    let card = vbox(6, &[header.upcast_ref(), list_stack.upcast_ref()]);
    card.add_css_class("content");
    window.set_child(Some(&stage::Stage::new(&card, CARD_SIZE)));

    let outside = gtk::GestureClick::new();
    let (weak_window, weak_card) = (window.downgrade(), card.downgrade());
    outside.connect_pressed(move |_, _, x, y| {
        let (Some(window), Some(card)) = (weak_window.upgrade(), weak_card.upgrade()) else {
            return;
        };
        let point = gtk::graphene::Point::new(x as f32, y as f32);
        if card
            .compute_bounds(&window)
            .is_some_and(|b| !b.contains_point(&point))
        {
            window.close();
        }
    });
    window.add_controller(outside);

    let ui = Rc::new(Ui {
        window,
        search,
        status,
        model,
        selection,
        list,
        list_stack,
        empty,
        apps,
        history: RefCell::new(history),
        ranker: RefCell::new(Ranker::new()),
        expanded: Cell::new(None),
        pointer: Cell::new(None),
        pointer_moved: Cell::new(false),
        deferred: RefCell::new(Vec::new()),
    });
    ui.list.set_factory(Some(&row_factory(Rc::downgrade(&ui))));
    connect_signals(&ui);
    // On its first layout ListView can't know the row height yet and builds a
    // row for every item (80 rows ≈ 20 ms). Only the visible rows go into the
    // first frame; the rest are appended right after it is painted.
    ui.refresh();
    // Handlers hold weak references; the window owns the state until it is
    // destroyed. Closing it closes the glass too, so no windows are left and the
    // process exits.
    let owner = ui.clone();
    ui.window.connect_destroy(move |_| {
        let _ = &owner;
    });
    ui.window.connect_close_request(|window| {
        if let Some(app) = window.application() {
            app.windows()
                .iter()
                .filter(|w| *w != window)
                .for_each(|w| w.close());
        }
        glib::Propagation::Proceed
    });
    mark("built");
    // The glass maps first so it stacks below the content.
    glass.present();
    ui.window.present();
    ui.search.grab_focus();
    if let Some(clock) = glass.frame_clock() {
        once_after_paint(&clock, || mark("glass painted"));
    }
    if let Some(clock) = ui.window.frame_clock() {
        let weak = Rc::downgrade(&ui);
        once_after_paint(&clock, move || {
            mark("first frame");
            if let Some(ui) = weak.upgrade() {
                let rest = ui.deferred.take();
                ui.model.splice(ui.model.n_items(), 0, &rest);
            }
        });
    }
}

/// Runs `f` after the next frame is painted (and committed to the compositor).
fn once_after_paint(clock: &gdk::FrameClock, f: impl FnOnce() + 'static) {
    let handler = Rc::new(Cell::new(None));
    let f = Cell::new(Some(f));
    let id = clock.connect_after_paint({
        let handler = handler.clone();
        move |clock| {
            if let Some(f) = f.take() {
                f();
            }
            if let Some(id) = handler.take() {
                clock.disconnect(id);
            }
        }
    });
    handler.set(Some(id));
}

fn connect_signals(ui: &Rc<Ui>) {
    // `changed` rather than `search-changed`, which waits 150 ms.
    let weak = Rc::downgrade(ui);
    ui.search.connect_changed(move |_| {
        let Some(ui) = weak.upgrade() else { return };
        let typed = Instant::now();
        ui.expanded.set(None);
        ui.refresh();
        let ranked = typed.elapsed();
        if let Some(clock) = ui.window.frame_clock() {
            let layout_start = Rc::new(Cell::new(None));
            let id = Rc::new(Cell::new(None));
            let (ls, id2) = (layout_start.clone(), id.clone());
            id.set(Some(clock.connect_layout(move |clock| {
                ls.set(Some(typed.elapsed()));
                if let Some(id) = id2.take() {
                    clock.disconnect(id);
                }
            })));
            once_after_paint(&clock, move || {
                let waited = layout_start.get().unwrap_or_default();
                mark(&format!(
                    "typed→painted {:.2} ms (results {:.2}, wait {:.2}, layout+paint {:.2})",
                    typed.elapsed().as_secs_f64() * 1000.0,
                    ranked.as_secs_f64() * 1000.0,
                    (waited - ranked).as_secs_f64() * 1000.0,
                    (typed.elapsed() - waited).as_secs_f64() * 1000.0,
                ));
            });
        }
    });

    // Hover selection: the window sees motion first (capture phase) and notes
    // whether the pointer really moved; rows then select themselves.
    let motion = gtk::EventControllerMotion::new();
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(ui);
    motion.connect_enter(move |_, x, y| {
        if let Some(ui) = weak.upgrade() {
            ui.pointer.set(Some((x, y)));
        }
    });
    let weak = Rc::downgrade(ui);
    motion.connect_motion(move |_, x, y| {
        if let Some(ui) = weak.upgrade() {
            let moved = ui.pointer.replace(Some((x, y))) != Some((x, y));
            ui.pointer_moved.set(moved);
        }
    });
    ui.window.add_controller(motion);

    // Keys are handled before the focused widget sees them, so the search
    // entry keeps focus while arrows, Tab and Enter drive the list.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(ui);
    keys.connect_key_pressed(move |_, key, _, mods| {
        let Some(ui) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        ui.on_key(key, mods.contains(gdk::ModifierType::CONTROL_MASK))
    });
    ui.window.add_controller(keys);
    // Added after the key handler so it sees Esc, arrows and Enter first; other
    // typing goes to the search entry.
    ui.search.set_key_capture_widget(Some(&ui.window));
}

impl Ui {
    fn on_key(&self, key: gdk::Key, ctrl: bool) -> glib::Propagation {
        let page = VISIBLE_ROWS - 1;
        match key {
            gdk::Key::Escape => self.window.close(),
            gdk::Key::Down => self.move_selection(1),
            gdk::Key::Up => self.move_selection(-1),
            gdk::Key::Page_Down => self.move_selection(page),
            gdk::Key::Page_Up => self.move_selection(-page),
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => {
                self.activate(self.selection.selected(), ctrl)
            }
            gdk::Key::Tab => self.toggle_actions(),
            gdk::Key::ISO_Left_Tab => {
                self.collapse();
            }
            gdk::Key::Right if self.caret_at_end() && self.expand_selected() => {}
            gdk::Key::Left if self.in_actions() => {
                self.collapse();
            }
            _ => match key.to_unicode().and_then(|c| c.to_digit(10)) {
                Some(n @ 1..=9) if ctrl => self.activate(n - 1, false),
                _ => return glib::Propagation::Proceed,
            },
        }
        glib::Propagation::Stop
    }

    fn caret_at_end(&self) -> bool {
        self.search.position() >= self.search.text().chars().count() as i32
    }

    /// Rebuilds the list for the current search text.
    fn refresh(&self) {
        let text = self.search.text();
        let mut items = Vec::new();
        let empty_text = if let Some(expr) = text.strip_prefix('=') {
            if let Some(value) = calc::eval(expr) {
                items.push(Item::Calc {
                    expr: expr.trim().to_string(),
                    value: calc::format(value),
                });
            }
            "Type a calculation, like =12*4.5"
        } else if let Some(command) = text.strip_prefix('>') {
            if !command.trim().is_empty() {
                items.push(Item::Command(command.trim().to_string()));
            }
            "Type a command to run"
        } else {
            let order = self.ranker.borrow_mut().rank(
                &self.apps,
                &self.history.borrow(),
                &text,
                history::now(),
            );
            items.extend(order.into_iter().take(MAX_RESULTS).map(Item::App));
            if self.apps.is_empty() {
                "No apps found"
            } else {
                "No matches"
            }
        };
        let mut objects: Vec<BoxedAnyObject> = items.into_iter().map(BoxedAnyObject::new).collect();
        // Before the first frame, keep back all but the visible rows.
        let first_fill = self.model.n_items() == 0 && self.window.frame_clock().is_none();
        let rest = if first_fill && objects.len() > VISIBLE_ROWS as usize {
            objects.split_off(VISIBLE_ROWS as usize)
        } else {
            Vec::new()
        };
        self.deferred.replace(rest);
        self.model.splice(0, self.model.n_items(), &objects);
        self.empty.set_label(empty_text);
        self.list_stack
            .set_visible_child_name(if objects.is_empty() { "empty" } else { "list" });
        self.select(0);
    }

    fn with_item<T>(&self, position: u32, f: impl FnOnce(&Item) -> T) -> Option<T> {
        let object = self
            .model
            .item(position)?
            .downcast::<BoxedAnyObject>()
            .ok()?;
        let item = object.borrow::<Item>();
        Some(f(&item))
    }

    fn select(&self, position: u32) {
        if position < self.model.n_items() {
            self.selection.set_selected(position);
            self.list
                .scroll_to(position, gtk::ListScrollFlags::NONE, None);
        }
    }

    fn move_selection(&self, delta: i32) {
        let n = self.model.n_items() as i32;
        if n == 0 {
            return;
        }
        let current = match self.selection.selected() {
            gtk::INVALID_LIST_POSITION => 0,
            p => p as i32,
        };
        self.select((current + delta).clamp(0, n - 1) as u32);
    }

    /// Position of an app's own row.
    fn position_of(&self, app: usize) -> Option<u32> {
        (0..self.model.n_items()).find(|&i| {
            self.with_item(i, |item| matches!(item, Item::App(a) if *a == app)) == Some(true)
        })
    }

    fn in_actions(&self) -> bool {
        self.expanded.get().is_some()
            && self
                .with_item(self.selection.selected(), |item| match item {
                    Item::Action { .. } => true,
                    Item::App(app) => self.expanded.get() == Some(*app),
                    _ => false,
                })
                .unwrap_or(false)
    }

    /// Tab: shows the selected app's actions, or hides them again.
    fn toggle_actions(&self) {
        if self.in_actions() {
            self.collapse();
        } else {
            self.expand_selected();
        }
    }

    /// Shows the selected app's desktop actions below it and selects the first.
    fn expand_selected(&self) -> bool {
        let Some(Some(app)) = self.with_item(self.selection.selected(), |item| match item {
            Item::App(app) => Some(*app),
            _ => None,
        }) else {
            return false;
        };
        if !self.apps[app].has_actions || self.expanded.get() == Some(app) {
            return false;
        }
        self.collapse();
        let actions = apps::actions(&self.apps[app].id);
        let Some(position) = self.position_of(app).filter(|_| !actions.is_empty()) else {
            return false;
        };
        let objects: Vec<BoxedAnyObject> = actions
            .into_iter()
            .map(|action| {
                BoxedAnyObject::new(Item::Action {
                    app,
                    id: action.id,
                    name: action.name,
                })
            })
            .collect();
        self.model.splice(position + 1, 0, &objects);
        self.expanded.set(Some(app));
        self.select(position + 1);
        true
    }

    /// Hides the shown actions and selects their app.
    fn collapse(&self) -> bool {
        let Some(app) = self.expanded.take() else {
            return false;
        };
        let Some(position) = self.position_of(app) else {
            return false;
        };
        let count = (position + 1..self.model.n_items())
            .take_while(|&i| {
                self.with_item(i, |item| matches!(item, Item::Action { .. })) == Some(true)
            })
            .count();
        self.model
            .splice(position + 1, count as u32, &[] as &[BoxedAnyObject]);
        self.select(position);
        true
    }

    /// Enter, a click or Ctrl+N: launch, run or copy, then close.
    fn activate(&self, position: u32, ctrl: bool) {
        let Some((launch, used)) = self.with_item(position, |item| match item {
            Item::App(app) => (Target::App(*app, None), Some(*app)),
            Item::Action { app, id, .. } => (Target::App(*app, Some(id.clone())), Some(*app)),
            Item::Calc { value, .. } => (Target::Copy(value.clone()), None),
            Item::Command(command) => (Target::Shell(command.clone()), None),
        }) else {
            return;
        };
        let what = match &launch {
            Target::App(app, action) => Launch::App {
                id: &self.apps[*app].id,
                action: action.as_deref(),
            },
            Target::Shell(command) => Launch::Shell {
                command,
                terminal: ctrl,
            },
            Target::Copy(value) => Launch::Copy(value),
        };
        let context = WidgetExt::display(&self.window).app_launch_context();
        if let Err(e) = launch::run(what, context.upcast_ref()) {
            self.show_error(&format!("{e:#}"));
            return;
        }
        if let Some(app) = used {
            let mut history = self.history.borrow_mut();
            history.record(&self.apps[app].id, history::now());
            if let Err(e) = history.save() {
                eprintln!("pixie: saving history: {e:#}");
            }
        }
        self.window.close();
    }

    fn show_error(&self, message: &str) {
        eprintln!("pixie: {message}");
        self.status.set_label(message);
        self.status.set_tooltip_text(Some(message));
        self.status.set_visible(true);
    }
}

/// What an activated row starts, detached from the borrowed list item.
enum Target {
    App(usize, Option<String>),
    Shell(String),
    Copy(String),
}

fn row_factory(ui: Weak<Ui>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let setup_ui = ui.clone();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let icon = gtk::Image::builder()
            .pixel_size(ICON_SIZE)
            .css_classes(["icon"])
            .build();
        // max_width_chars(1) keeps long text from widening the card; it ellipsizes instead.
        let label = |class: &str| {
            gtk::Label::builder()
                .xalign(0.0)
                .single_line_mode(true)
                .max_width_chars(1)
                .hexpand(true)
                .ellipsize(pango::EllipsizeMode::End)
                .css_classes([class])
                .build()
        };
        let text = vbox(
            1,
            &[label("title").upcast_ref(), label("subtitle").upcast_ref()],
        );
        text.set_valign(gtk::Align::Center);
        text.set_hexpand(true);
        let row = hbox(12, &[icon.upcast_ref(), text.upcast_ref()]);
        row.add_css_class("row-content");

        // Hover selects (only after real pointer movement), a click activates.
        let motion = gtk::EventControllerMotion::new();
        let (weak_ui, weak_item) = (setup_ui.clone(), item.downgrade());
        let select = move || {
            if let (Some(ui), Some(item)) = (weak_ui.upgrade(), weak_item.upgrade())
                && ui.pointer_moved.get()
                && item.position() != ui.selection.selected()
            {
                ui.selection.set_selected(item.position());
            }
        };
        let on_enter = select.clone();
        motion.connect_enter(move |_, _, _| on_enter());
        motion.connect_motion(move |_, _, _| select());
        row.add_controller(motion);

        let click = gtk::GestureClick::builder().button(1).build();
        let (weak_ui, weak_item) = (setup_ui.clone(), item.downgrade());
        click.connect_released(move |gesture, presses, x, y| {
            let inside = gesture.widget().is_some_and(|w| w.contains(x, y));
            if let (Some(ui), Some(item)) = (weak_ui.upgrade(), weak_item.upgrade())
                && presses == 1
                && inside
            {
                ui.activate(item.position(), false);
            }
        });
        row.add_controller(click);
        item.set_child(Some(&row));
    });
    factory.connect_bind(move |_, item| {
        let Some(ui) = ui.upgrade() else { return };
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let object = item.item().and_downcast::<BoxedAnyObject>().unwrap();
        let entry = object.borrow::<Item>();
        let row = item.child().and_downcast::<gtk::Box>().unwrap();
        let theme = gtk::IconTheme::for_display(&WidgetExt::display(&row));
        let icon = row.first_child().and_downcast::<gtk::Image>().unwrap();
        let text = row.last_child().unwrap();
        let title = text.first_child().and_downcast::<gtk::Label>().unwrap();
        let subtitle = text.last_child().and_downcast::<gtk::Label>().unwrap();

        let (gicon, size, title_text, subtitle_text, kind): (gio::Icon, i32, &str, &str, &str) =
            match &*entry {
                Item::App(i) => {
                    let app = &ui.apps[*i];
                    (
                        apps::icon(app.icon.as_deref(), &theme),
                        ICON_SIZE,
                        &app.name,
                        &app.subtitle,
                        "app",
                    )
                }
                Item::Action { app, name, .. } => {
                    let app = &ui.apps[*app];
                    (
                        apps::icon(app.icon.as_deref(), &theme),
                        ACTION_ICON_SIZE,
                        name,
                        "",
                        "action",
                    )
                }
                Item::Calc { expr, value } => (
                    gio::ThemedIcon::new("accessories-calculator").upcast(),
                    ICON_SIZE,
                    value,
                    expr,
                    "calc",
                ),
                Item::Command(command) => (
                    gio::ThemedIcon::new("utilities-terminal").upcast(),
                    ICON_SIZE,
                    command,
                    "Enter to run, Ctrl+Enter to run in a terminal",
                    "command",
                ),
            };
        icon.set_pixel_size(size);
        icon.set_from_gicon(&gicon);
        title.set_label(title_text);
        subtitle.set_label(subtitle_text);
        subtitle.set_visible(!subtitle_text.is_empty());
        row.set_css_classes(&["row-content", kind]);
    });
    factory
}

fn hbox(spacing: i32, children: &[&gtk::Widget]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, spacing);
    children.iter().for_each(|c| b.append(*c));
    b
}

fn vbox(spacing: i32, children: &[&gtk::Widget]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, spacing);
    children.iter().for_each(|c| b.append(*c));
    b
}
