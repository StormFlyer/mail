use i_slint_core::accessibility::AccessibleStringProperty;
use i_slint_core::item_tree::ItemRc;
use i_slint_core::window::WindowInner;
use slint::platform::{
    Platform, PointerEventButton, WindowAdapter, WindowEvent,
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::Cell, ops::ControlFlow, rc::Rc};

slint::slint! {
    import { EmailRow, MailLabelRow, MailListEntry } from "../ui/models.slint";
    import { MailListPane } from "../ui/views/mail-list-pane.slint";
    import { MotionSettings } from "../ui/design-system.slint";

    export component DesktopListHarness inherits Window {
        preferred-width: 397px;
        preferred-height: 260px;
        in-out property <[EmailRow]> emails: [];
        in-out property <[MailListEntry]> entries: [];
        init => { MotionSettings.enabled = false; }

        MailListPane {
            width: parent.width;
            height: parent.height;
            emails: root.emails;
            list_entries: root.entries;
            surface: #18191a;
            heading: white;
            body-text: white;
            secondary-text: #cccccc;
            muted-text: #999999;
            border: #3a3b3c;
            border-subtle: #303030;
            accent: #5ca3ff;
        }
    }

    export component MailListHarness inherits Window {
        preferred-width: 397px;
        preferred-height: 480px;
        in-out property <[EmailRow]> emails: [];
        in-out property <[MailListEntry]> entries: [];
        in-out property <bool> more: false;
        in-out property <bool> paging: false;
        in-out property <int> revision: 0;
        callback group-toggled();
        callback page-requested();
        init => { MotionSettings.enabled = false; }

        MailListPane {
            width: parent.width;
            height: parent.height;
            compact: true;
            emails: root.emails;
            list_entries: root.entries;
            can_load_more: root.more;
            loading_more: root.paging;
            content_revision: root.revision;
            surface: #18191a;
            heading: white;
            body-text: white;
            secondary-text: #cccccc;
            muted-text: #999999;
            border-subtle: #303030;
            accent: #5ca3ff;
            toggle_mail_group(key) => { root.group-toggled(); }
            load_more => { root.page-requested(); }
        }
    }
}

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

#[test]
fn toggling_a_date_group_keeps_infinite_pagination_active() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
    let ui = MailListHarness::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(397, 480));
    ui.set_emails(ModelRc::new(VecModel::from(vec![EmailRow {
        id: 1,
        sender: "Sender".into(),
        subject: "Subject".into(),
        ..Default::default()
    }])));
    ui.set_entries(ModelRc::new(VecModel::from(vec![
        MailListEntry {
            is_header: true,
            group_key: "this-week".into(),
            group_kind: "this-week".into(),
            group_count: 1,
            expanded: true,
            show_row: true,
            ..Default::default()
        },
        MailListEntry {
            show_row: true,
            email_index: 0,
            email: EmailRow {
                id: 1,
                sender: "Sender".into(),
                subject: "Subject".into(),
                ..Default::default()
            },
            ..Default::default()
        },
    ])));
    let toggles = Rc::new(Cell::new(0));
    let pages = Rc::new(Cell::new(0));
    let count = toggles.clone();
    ui.on_group_toggled(move || count.set(count.get() + 1));
    let count = pages.clone();
    ui.on_page_requested(move || count.set(count.get() + 1));
    ui.show().unwrap();

    let position = slint::LogicalPosition::new(80.0, 82.0);
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
    assert_eq!(
        toggles.get(),
        1,
        "the date group should respond to the click"
    );

    ui.set_more(true);
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        pages.get(),
        1,
        "pagination should still load after a group click"
    );

    // Finish collapsing all loaded mail, then commit multiple pages whose
    // messages belong to the same collapsed section. No scroll gesture or
    // change in visible height should be needed to reach the older mail.
    ui.set_more(false);
    ui.set_entries(ModelRc::new(VecModel::from(vec![MailListEntry {
        is_header: true,
        group_key: "this-week".into(),
        group_kind: "this-week".into(),
        group_count: 1,
        expanded: false,
        ..Default::default()
    }])));
    let settle = || {
        let mut pixels = vec![slint::Rgb8Pixel::default(); 397 * 480];
        for _ in 0..8 {
            slint::platform::update_timers_and_animations();
            window.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, 397);
            });
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };
    settle();
    for revision in 1..=3 {
        ui.set_paging(true);
        ui.set_emails(ModelRc::new(VecModel::from(
            (1..=revision + 1)
                .map(|id| EmailRow {
                    id,
                    ..Default::default()
                })
                .collect::<Vec<_>>(),
        )));
        ui.set_revision(revision);
        ui.set_paging(false);
        ui.set_more(true);
        settle();
        assert_eq!(
            pages.get(),
            revision + 1,
            "pages hidden in a collapsed section must not stall pagination"
        );
    }
    ui.set_more(false);
    ui.set_revision(4);
    settle();
    assert_eq!(pages.get(), 4, "stop when the mailbox is exhausted");
}

struct Desktop(Rc<MinimalSoftwareWindow>);

impl Platform for Desktop {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

/// One desktop list showing `email`.
fn desktop_list(email: EmailRow) -> DesktopListHarness {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(Desktop(window.clone()))).unwrap();
    let ui = DesktopListHarness::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(397, 260));
    ui.set_emails(ModelRc::new(VecModel::from(vec![email.clone()])));
    ui.set_entries(ModelRc::new(VecModel::from(vec![MailListEntry {
        show_row: true,
        email_index: 0,
        email,
        ..Default::default()
    }])));
    ui.show().unwrap();
    let mut pixels = vec![slint::Rgb8Pixel::default(); 397 * 260];
    for _ in 0..3 {
        slint::platform::update_timers_and_animations();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, 397);
        });
    }
    ui
}

fn label(id: i32, name: &str) -> MailLabelRow {
    MailLabelRow {
        id,
        name: name.into(),
        color: slint::Color::from_rgb_u8(0x4a, 0x86, 0xe8),
        applied: true,
        ..Default::default()
    }
}

/// The shown element with this accessible label.
fn labelled(window: &slint::Window, label: &str) -> Option<ItemRc> {
    let inner = WindowInner::from_pub(window);
    ItemRc::new_root(inner.component()).visit_descendants(|item| {
        if item.is_visible()
            && item
                .accessible_string_property(AccessibleStringProperty::Label)
                .as_deref()
                == Some(label)
        {
            ControlFlow::Break(item.clone())
        } else {
            ControlFlow::Continue(())
        }
    })
}

/// Left edge, top, width and height in the window.
fn bounds(item: &ItemRc) -> (f32, f32, f32, f32) {
    let geometry = item.geometry();
    let origin = item.map_to_window(geometry.origin);
    (origin.x, origin.y, geometry.width(), geometry.height())
}

#[test]
fn labels_past_the_limit_take_no_room() {
    let ui = desktop_list(EmailRow {
        id: 1,
        sender: "Cloudflare".into(),
        subject: "Your weekly traffic report".into(),
        preview: "Requests were up 18% across flectar.com".into(),
        labels: ModelRc::new(VecModel::from(vec![
            label(1, "Reports"),
            label(2, "Marketing"),
            label(3, "Travel"),
        ])),
        ..Default::default()
    });
    let (marketing_x, _, marketing_width, _) =
        bounds(&labelled(ui.window(), "Marketing").expect("the second label"));
    let (more_x, ..) = bounds(&labelled(ui.window(), "+1").expect("the label count"));
    assert!(labelled(ui.window(), "Travel").is_none());
    // The count follows the second chip's end padding and one gap; a hidden
    // third chip would push it further right.
    let gap = more_x - (marketing_x + marketing_width);
    assert!(gap < 10.0, "hidden labels took {gap}px");
}
