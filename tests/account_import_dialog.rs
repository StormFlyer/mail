use slint::platform::{
    Platform, PointerEventButton, WindowAdapter, WindowEvent,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
use slint::{Model, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

slint::slint! {
    import { AccountImport, ImportDialog } from "../ui/dialogs/import-dialog.slint";
    export { AccountImport, ImportAccountRow, ImportFindingRow, ImportProfileRow, ImportFailureRow } from "../ui/dialogs/import-dialog.slint";

    export component ImportHarness inherits Window {
        preferred-width: 800px;
        preferred-height: 700px;
        in property <bool> compact: false;
        out property <bool> behind-focused: behind.has-focus;
        callback open-accounts();

        // A control behind the dialog, which keyboard focus must not reach.
        behind := FocusScope {
            width: 10px;
            height: 10px;
        }

        ImportDialog {
            width: parent.width;
            height: parent.height;
            compact: root.compact;
            open-accounts => { root.open-accounts(); }
        }
    }
}

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn click(ui: &ImportHarness, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn finding(code: &str, detail: &str, blocking: bool) -> ImportFindingRow {
    ImportFindingRow {
        code: code.into(),
        detail: detail.into(),
        count: detail.parse().unwrap_or_default(),
        blocking,
    }
}

fn rows() -> Vec<ImportAccountRow> {
    vec![
        ImportAccountRow {
            email: "jane@example.com".into(),
            label: "Work".into(),
            server: "imap.example.com".into(),
            status: "ready".into(),
            sign_in: "password".into(),
            selected: true,
            findings: ModelRc::new(VecModel::from(vec![finding(
                "password-mechanism-changed",
                "",
                false,
            )])),
        },
        ImportAccountRow {
            email: "jane.doe@gmail.com".into(),
            label: "Gmail".into(),
            server: "imap.gmail.com".into(),
            status: "ready".into(),
            sign_in: "google".into(),
            selected: true,
            findings: ModelRc::new(VecModel::from(vec![finding(
                "aliases-managed-by-provider",
                "2",
                false,
            )])),
        },
        ImportAccountRow {
            email: "old@example.net".into(),
            label: "Old POP".into(),
            status: "unsupported".into(),
            sign_in: "password".into(),
            findings: ModelRc::new(VecModel::from(vec![
                finding("pop3-unsupported", "pop.example.net", true),
                finding("unencrypted-outgoing", "smtp.example.net", true),
            ])),
            ..Default::default()
        },
        ImportAccountRow {
            email: "me@example.org".into(),
            status: "already-added".into(),
            sign_in: "password".into(),
            ..Default::default()
        },
    ]
}

/// Renders a few frames; writes a PNG when `FLECTAR_IMPORT_SNAPSHOTS` names
/// a directory, for reviewing layout changes.
fn render(window: &MinimalSoftwareWindow, width: u32, height: u32, name: &str) {
    let mut pixels = vec![slint::Rgb8Pixel::default(); (width * height) as usize];
    for _ in 0..4 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width as usize);
        });
    }
    if let Some(directory) = std::env::var_os("FLECTAR_IMPORT_SNAPSHOTS") {
        let bytes = pixels
            .iter()
            .flat_map(|pixel| [pixel.r, pixel.g, pixel.b])
            .collect::<Vec<_>>();
        image::RgbImage::from_raw(width, height, bytes)
            .unwrap()
            .save(std::path::Path::new(&directory).join(format!("{name}.png")))
            .unwrap();
    }
}

#[test]
fn review_selection_import_and_completion_flow() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
    let ui = ImportHarness::new().unwrap();
    let state = ui.global::<AccountImport>();
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    for (name, events) in [("confirm", events.clone()), ("close", events.clone())] {
        let record = move || events.borrow_mut().push(name.to_owned());
        match name {
            "confirm" => state.on_confirm(record),
            _ => state.on_close(record),
        }
    }
    let toggles = events.clone();
    state.on_toggle(move |index, checked| {
        toggles
            .borrow_mut()
            .push(format!("toggle {index} {checked}"));
    });
    let file = events.clone();
    state.on_choose_file(move || file.borrow_mut().push("file".into()));
    let opened = events.clone();
    ui.on_open_accounts(move || opened.borrow_mut().push("open-accounts".into()));
    ui.show().unwrap();

    state.set_open(true);
    state.set_folder_supported(true);
    state.set_profiles(ModelRc::new(VecModel::from(vec![ImportProfileRow {
        name: "default-release".into(),
        path: "/home/me/.thunderbird/abcd.default-release".into(),
        is_default: true,
    }])));
    window.set_size(slint::PhysicalSize::new(800, 700));
    render(&window, 800, 700, "choose-desktop");

    state.set_source("k9".into());
    state.set_accounts(ModelRc::new(VecModel::from(rows())));
    state.set_selected_count(2);
    state.set_local_folders(1);
    state.set_stage("review".into());
    render(&window, 800, 700, "review-desktop");

    // Escape is the keyboard way out of the review step.
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Escape.into(),
    });
    assert_eq!(events.borrow().last().map(String::as_str), Some("close"));

    // While importing, the dialog cannot be dismissed.
    state.set_stage("importing".into());
    render(&window, 800, 700, "importing-desktop");
    events.borrow_mut().clear();
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Escape.into(),
    });
    assert!(events.borrow().is_empty());

    state.set_imported(2);
    state.set_already_present(1);
    state.set_failures(ModelRc::new(VecModel::from(vec![ImportFailureRow {
        email: "broken@example.com".into(),
        error: "incomplete IMAP settings".into(),
    }])));
    state.set_stage("done".into());
    render(&window, 800, 700, "done-desktop");

    // Phone layout: full-screen card, same content.
    window.set_size(slint::PhysicalSize::new(390, 780));
    ui.set_compact(true);
    state.set_stage("review".into());
    render(&window, 390, 780, "review-compact");
    state.set_stage("error".into());
    state.set_error_code("unknown-format".into());
    render(&window, 390, 780, "error-compact");
    assert_eq!(state.get_accounts().row_count(), 4);
    clicks_reach_the_selection_and_primary_actions(&ui, &window);
}

fn clicks_reach_the_selection_and_primary_actions(
    ui: &ImportHarness,
    window: &MinimalSoftwareWindow,
) {
    let state = ui.global::<AccountImport>();
    ui.set_compact(false);
    let toggled = Rc::new(RefCell::new(Vec::new()));
    let record = toggled.clone();
    state.on_toggle(move |index, checked| record.borrow_mut().push((index, checked)));
    let confirmed = Rc::new(RefCell::new(0));
    let count = confirmed.clone();
    state.on_confirm(move || *count.borrow_mut() += 1);
    window.set_size(slint::PhysicalSize::new(800, 700));
    state.set_open(true);
    state.set_accounts(ModelRc::new(VecModel::from(rows())));
    state.set_selected_count(2);
    state.set_stage("review".into());
    render(window, 800, 700, "click-review");

    // Card: 600 wide, centered; padding 22; first row checkbox.
    let card_x = (800.0 - 600.0) / 2.0;
    let card_y = (700.0 - 640.0) / 2.0;
    let mut hit = None;
    for y in (card_y as i32 + 60..card_y as i32 + 200).step_by(4) {
        click(ui, card_x + 22.0 + 10.0 + 16.0, y as f32);
        if !toggled.borrow().is_empty() {
            hit = Some(y);
            break;
        }
    }
    assert!(hit.is_some(), "the first row's checkbox is clickable");
    assert_eq!(toggled.borrow()[0], (0, false));

    // The primary button sits in the footer's right corner.
    for x in (card_x as i32 + 300..card_x as i32 + 578).rev().step_by(8) {
        click(ui, x as f32, card_y + 640.0 - 22.0 - 12.0);
        if *confirmed.borrow() > 0 {
            break;
        }
    }
    assert_eq!(*confirmed.borrow(), 1, "the import button confirms");

    // Escape still closes after a control inside the dialog took focus.
    let closed = Rc::new(RefCell::new(0));
    let count = closed.clone();
    state.on_close(move || *count.borrow_mut() += 1);
    state.set_stage("review".into());
    render(window, 800, 700, "click-review-again");
    click(ui, card_x + 22.0 + 10.0 + 16.0, hit.unwrap() as f32);
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Escape.into(),
    });
    assert_eq!(*closed.borrow(), 1, "Escape closes from inside the card");

    // A new step moves focus back to the dialog itself.
    state.set_stage("done".into());
    render(window, 800, 700, "click-done");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Escape.into(),
    });
    assert_eq!(*closed.borrow(), 2, "Escape closes after the step changed");

    // Tab and Shift+Tab cycle inside the dialog in both directions.
    state.set_stage("review".into());
    render(window, 800, 700, "tab-review");
    for (key, name) in [
        (slint::platform::Key::Tab, "Tab"),
        (slint::platform::Key::Backtab, "Shift+Tab"),
    ] {
        for _ in 0..12 {
            ui.window()
                .dispatch_event(WindowEvent::KeyPressed { text: key.into() });
            ui.window()
                .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
            assert!(!ui.get_behind_focused(), "{name} left the dialog");
        }
    }
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Escape.into(),
    });
    assert_eq!(*closed.borrow(), 3, "Escape closes after cycling with Tab");
}
