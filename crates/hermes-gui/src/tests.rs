//! End-to-end against a real `hermes-server`, without a window: the add-instance steps (invite link
//! with a fingerprint → create the owner account; plain address → trust on first use → sign in; a
//! changed certificate shows the warning) and the message rows following the store.

use std::sync::Arc;
use std::time::Duration;

use hermes_client_core::address::{Fingerprint, HostPort};
use hermes_client_core::config::ClientConfig;
use hermes_client_core::proto::api::{CreateInviteRequest, RegistrationMode as Registration};
use hermes_client_core::proto::id::Id;
use hermes_client_core::{CertStatus, Core, CoreEvents, CoreOptions, Draft};
use hermes_server::config::{Config, RegistrationMode, TlsMode};
use hermes_server::state::AppState;
use tokio::sync::Notify;

use crate::add_instance::{self, Account, Probed, Tab};
use crate::menu_entries::Target;
use crate::rows::{self, Row};

struct Server {
    link: String,
    host: HostPort,
    shutdown: Arc<Notify>,
    dir: std::path::PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.notify_one();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start(name: &str) -> Server {
    let dir = std::env::temp_dir().join(format!("hermes-gui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = tcp.local_addr().unwrap().port();
    let mut config = Config::default();
    config.instance.name = "Friends".into();
    config.instance.description = "Our place".into();
    config.instance.public_host = "127.0.0.1".into();
    config.network.port = port;
    config.storage.data_dir = dir.join("data");
    config.tls.mode = TlsMode::SelfSigned;
    config.registration.mode = RegistrationMode::InviteOnly;
    config.privacy.require_channel_encryption = false;
    let tls = hermes_server::tls::load(&config).unwrap();
    let fp = *tls.fingerprint().unwrap();
    let mut state = AppState::load(config).await.unwrap();
    state.fingerprint = Some(fp);
    let links = hermes_server::setup::first_start(&state, Some(&fp))
        .await
        .unwrap()
        .unwrap();
    let shutdown = Arc::new(Notify::new());
    let signal = {
        let shutdown = shutdown.clone();
        async move { shutdown.notified().await }
    };
    tokio::spawn(hermes_server::server::run(
        Arc::new(state),
        tcp,
        tls,
        None,
        signal,
    ));
    Server {
        link: links.web,
        host: HostPort::parse(&format!("127.0.0.1:{port}")).unwrap(),
        shutdown,
        dir,
    }
}

fn client() -> (Core, CoreEvents) {
    Core::new(
        ClientConfig::default(),
        CoreOptions::in_memory("hermes-gui"),
    )
}

fn account(probed: &Probed, tab: Tab, username: &str) -> Account {
    Account {
        host: probed.probe.host.clone(),
        name: probed.probe.info.name.clone(),
        description: probed.probe.info.description.clone(),
        registration: probed.probe.info.registration_mode,
        invite: probed.invite.clone(),
        preview: probed.preview.clone(),
        tab,
        username: username.to_owned(),
        password: "correct horse battery".to_owned(),
    }
}

/// Waits until the store of `inst` says so.
async fn eventually(core: &Core, what: &str, f: impl Fn(&hermes_client_core::Store) -> bool) {
    for _ in 0..200 {
        if f(&core.store()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never happened: {what}");
}

#[tokio::test]
async fn invite_link_creates_the_account_then_others_sign_in_and_join() {
    let server = start("invite").await;
    let (owner, _events) = client();

    // The link carries the fingerprint, so there is nothing to ask.
    let probed = add_instance::probe(owner.clone(), server.link.clone())
        .await
        .unwrap();
    assert_eq!(probed.probe.cert, CertStatus::Pinned);
    assert_eq!(probed.probe.info.name, "Friends");
    let inst = add_instance::submit(owner.clone(), account(&probed, Tab::Register, "alice"))
        .await
        .unwrap();
    eventually(&owner, "the owner is signed in", |s| {
        s.instance(&inst).is_some_and(|s| s.me.is_some())
    })
    .await;

    let rest = owner.rest(&inst).unwrap();
    let space = rest.create_space("Hangout").await.unwrap();
    let invite = rest
        .create_invite(space.space.id, &CreateInviteRequest::default())
        .await
        .unwrap();
    let link = owner.invite_link(&inst, &invite.code).unwrap();

    // Someone else, on a server they do not know yet: the link's fingerprint is enough, and it
    // shows what they are invited to.
    let (guest, _guest_events) = client();
    let probed = add_instance::probe(guest.clone(), link).await.unwrap();
    assert_eq!(
        probed.probe.info.registration_mode,
        Registration::InviteOnly
    );
    let preview = probed.preview.clone().expect("an invite preview");
    assert_eq!(preview.space.expect("its space").name, "Hangout");
    let guest_inst = add_instance::submit(guest.clone(), account(&probed, Tab::Register, "bob"))
        .await
        .unwrap();
    eventually(&guest, "the guest sees the space", |s| {
        s.instance(&guest_inst)
            .is_some_and(|s| s.spaces.contains_key(&space.space.id))
    })
    .await;

    // A wrong password is an error for the dialog, not a panic.
    let (third, _third_events) = client();
    let probed = add_instance::probe(third.clone(), server.host.display())
        .await
        .unwrap();
    assert_eq!(probed.probe.cert, CertStatus::Unknown, "trust on first use");
    third
        .trust_fingerprint(&probed.probe.host, probed.probe.fingerprint)
        .unwrap();
    let mut wrong = account(&probed, Tab::SignIn, "bob");
    wrong.password = "not it".into();
    assert!(add_instance::submit(third.clone(), wrong).await.is_err());
}

#[tokio::test]
async fn changed_certificate_shows_the_warning() {
    let server = start("changed").await;
    let (core, _events) = client();
    let pinned = Fingerprint::parse(&format!("sha256:{}", "00".repeat(32))).unwrap();
    core.trust_fingerprint(&server.host, pinned).unwrap();
    let probed = add_instance::probe(core.clone(), server.host.display())
        .await
        .unwrap();
    assert_eq!(probed.probe.cert, CertStatus::Changed { pinned });
}

#[tokio::test]
async fn bad_address_is_an_error_in_the_dialog() {
    let (core, _events) = client();
    assert!(
        add_instance::probe(core.clone(), "not an address at all".into())
            .await
            .is_err()
    );
    assert!(
        add_instance::probe(core, "127.0.0.1:1".into())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rows_follow_history_sends_and_deletes() {
    let server = start("rows").await;
    let (core, _events) = client();
    let probed = add_instance::probe(core.clone(), server.link.clone())
        .await
        .unwrap();
    let inst = add_instance::submit(core.clone(), account(&probed, Tab::Register, "alice"))
        .await
        .unwrap();
    eventually(&core, "signed in", |s| {
        s.instance(&inst).is_some_and(|s| s.me.is_some())
    })
    .await;
    let rest = core.rest(&inst).unwrap();
    let space = rest.create_space("Hangout").await.unwrap();
    let channel: Id = space
        .channels
        .iter()
        .find(|c| c.kind.has_messages())
        .expect("a text channel")
        .id;
    eventually(&core, "the channel arrives", |s| {
        s.instance(&inst)
            .is_some_and(|s| s.channel(channel).is_some())
    })
    .await;
    core.open_channel(&inst, channel).await.unwrap();

    let options = rows::Options {
        group_minutes: 7,
        utc_offset: 0,
        read_marker: None,
    };
    let messages = |core: &Core| -> Vec<Row> {
        let store = core.store();
        let s = store.instance(&inst).unwrap();
        rows::build(s, s.window(channel).unwrap(), &options)
            .into_iter()
            .filter(|r| matches!(r, Row::Message(_) | Row::Pending(_)))
            .collect()
    };
    assert!(messages(&core).is_empty());

    for text in ["one", "two"] {
        core.send_message(
            &inst,
            channel,
            Draft {
                content: text.into(),
                ..Draft::default()
            },
        )
        .unwrap();
    }
    assert_eq!(messages(&core).len(), 2, "pending ones show right away");
    eventually(&core, "both are sent", |s| {
        s.instance(&inst)
            .and_then(|s| s.window(channel))
            .is_some_and(|w| w.pending.is_empty() && w.messages.len() >= 2)
    })
    .await;
    let rows = messages(&core);
    let [Row::Message(first), Row::Message(second)] = rows.as_slice() else {
        panic!("two messages: {rows:?}");
    };
    assert!(!first.grouped);
    assert!(second.grouped, "the same author, right after");

    core.delete_message(&inst, channel, first.id).await.unwrap();
    eventually(&core, "the delete arrives", |s| {
        s.instance(&inst)
            .and_then(|s| s.window(channel))
            .and_then(|w| w.get(first.id))
            .is_some_and(|m| m.deleted)
    })
    .await;
    assert_eq!(messages(&core).len(), 1, "deleted messages are left out");
}

/// The ID of the `n`th loaded message of a channel.
fn visual_message(
    shell: &gpui_kit::Entity<crate::shell::Shell>,
    cx: &gpui_kit::App,
    inst: &str,
    channel: Id,
    n: usize,
) -> Id {
    let core = shell.read(cx).core.clone();
    let store = core.store();
    let window = store
        .instance(inst)
        .and_then(|s| s.window(channel))
        .unwrap();
    window.messages[n].id
}

/// gpui keeps a list's state mutably borrowed while it calls the scroll handler. Reading the list
/// from there panicked, which on Windows aborted the client at the first wheel notch.
#[gpui::test]
fn the_wheel_and_menus_over_a_chat_do_not_crash(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::component::{Theme, ThemeMode};
    use gpui_kit::{
        AppContext as _, Modifiers, ScrollDelta, ScrollEvent, VisualTestContext, WindowOptions,
        point, px,
    };

    // The real server and tokio wake the window's tasks from other threads.
    cx.executor().allow_parking();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    // As in `main`: the core spawns tokio tasks from the UI thread.
    let _guard = runtime.enter();
    let mut config = ClientConfig::default();
    config.general.check_for_updates = false;
    let (core, events) = Core::new(config, CoreOptions::in_memory("hermes-gui"));
    let (server, inst, space, channel) = runtime.block_on(async {
        let server = start("wheel").await;
        let probed = add_instance::probe(core.clone(), server.link.clone())
            .await
            .unwrap();
        let inst = add_instance::submit(core.clone(), account(&probed, Tab::Register, "alice"))
            .await
            .unwrap();
        eventually(&core, "signed in", |s| {
            s.instance(&inst).is_some_and(|s| s.me.is_some())
        })
        .await;
        let space = core
            .rest(&inst)
            .unwrap()
            .create_space("Hangout")
            .await
            .unwrap();
        let channel: Id = space
            .channels
            .iter()
            .find(|c| c.kind.has_messages())
            .expect("a text channel")
            .id;
        eventually(&core, "the channel arrives", |s| {
            s.instance(&inst)
                .is_some_and(|s| s.channel(channel).is_some())
        })
        .await;
        core.open_channel(&inst, channel).await.unwrap();
        core.send_message(
            &inst,
            channel,
            Draft {
                content: "hello".into(),
                ..Draft::default()
            },
        )
        .unwrap();
        eventually(&core, "the message is sent", |s| {
            s.instance(&inst)
                .and_then(|s| s.window(channel))
                .is_some_and(|w| w.pending.is_empty() && !w.messages.is_empty())
        })
        .await;
        let notes = server.dir.join("notes.txt");
        std::fs::write(&notes, "some notes").unwrap();
        let file = core
            .upload_to(&inst, channel, &notes, Default::default())
            .await
            .unwrap();
        core.send_message(
            &inst,
            channel,
            Draft {
                attachments: vec![file],
                ..Draft::default()
            },
        )
        .unwrap();
        eventually(&core, "the file is sent", |s| {
            s.instance(&inst)
                .and_then(|s| s.window(channel))
                .is_some_and(|w| w.pending.is_empty() && w.messages.len() == 2)
        })
        .await;
        let song = server.dir.join("song.flac");
        std::fs::write(&song, "not really a song").unwrap();
        let file = core
            .upload_to(&inst, channel, &song, Default::default())
            .await
            .unwrap();
        core.send_message(
            &inst,
            channel,
            Draft {
                attachments: vec![file],
                ..Draft::default()
            },
        )
        .unwrap();
        eventually(&core, "the sound is sent", |s| {
            s.instance(&inst)
                .and_then(|s| s.window(channel))
                .is_some_and(|w| w.pending.is_empty() && w.messages.len() == 3)
        })
        .await;
        (server, inst, space.space.id, channel)
    });

    let boot = crate::shell::Boot {
        core,
        events,
        runtime: runtime.handle().clone(),
        endpoint: crate::single_instance::Endpoint::for_config_dir(&server.dir.join("client")),
        link: None,
        warning: None,
        tray: None,
        global_keys: None,
        start_hidden: false,
    };
    let (window, shell) = cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| crate::shell::Shell::new(boot, window, cx))
        })
        .unwrap()
    });
    let mut visual = VisualTestContext::from_window(window, cx);
    let key = (inst.clone(), channel);
    visual.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.open_chat(inst.clone(), channel, window, cx);
        });
    });
    for _ in 0..200 {
        visual.run_until_parked();
        let ready = visual.update(|_, cx| {
            shell
                .read(cx)
                .chats
                .get(&key)
                .is_some_and(|view| !view.rows.is_empty())
        });
        if ready {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // Read marking only looks at the list while the window has the focus.
    visual.update(|window, cx| {
        shell.update(cx, |this, _| this.focused = true);
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let position = point(px(700.), px(300.));
    visual.simulate_mouse_move(position, None, Modifiers::none());
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    for lines in [1.0, -1.0] {
        visual.simulate_event(ScrollEvent {
            position,
            delta: ScrollDelta::Lines(point(0.0, lines)),
            ..Default::default()
        });
        visual.run_until_parked();
    }

    // Dragging over a message's text selects it, and is no click on the message.
    let message = visual.update(|_, cx| {
        let core = shell.read(cx).core.clone();
        let store = core.store();
        let window = store.instance(&inst).and_then(|s| s.window(channel));
        window.and_then(|w| w.messages.first().map(|m| m.id))
    });
    let part = crate::selection::Part {
        message: message.expect("a message"),
        index: crate::selection::BLOCKS,
    };
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let text = visual
        .update(|_, cx| shell.read(cx).selection.bounds_of(part))
        .expect("the message's text was painted");
    let (from, to) = (
        point(text.left() + px(1.), text.center().y),
        point(text.right() - px(1.), text.center().y),
    );
    visual.simulate_mouse_move(from, None, Modifiers::none());
    visual.simulate_mouse_down(from, gpui_kit::MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    visual.simulate_mouse_move(to, Some(gpui_kit::MouseButton::Left), Modifiers::none());
    visual.simulate_mouse_up(to, gpui_kit::MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    visual.update(|_, cx| {
        let this = shell.read(cx);
        assert!(this.selection.dragged, "the press became a drag");
        assert!(!this.selection.dragging(), "the release ended it");
        assert_eq!(this.selected_text().as_deref(), Some("hello"));
    });

    // A plain click elsewhere clears it.
    visual.simulate_mouse_move(position, None, Modifiers::none());
    visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    visual.simulate_mouse_down(position, gpui_kit::MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_up(position, gpui_kit::MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    visual.update(|_, cx| {
        assert_eq!(shell.read(cx).selected_text(), None);
    });

    // A right click on a file opens the file's menu, on a name the person's, not the message's.
    let (first, with_file) = visual.update(|_, cx| {
        let core = shell.read(cx).core.clone();
        let store = core.store();
        let window = store
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .unwrap();
        (window.messages[0].id, window.messages[1].clone())
    });
    let file_part = crate::selection::Part {
        message: with_file.id,
        index: crate::selection::FILES,
    };
    let author_part = crate::selection::Part {
        message: first,
        index: crate::selection::AUTHOR,
    };
    let right = gpui_kit::MouseButton::Right;
    let menu_target = Target::File(
        inst.clone(),
        channel,
        with_file.id,
        with_file.attachments[0].id,
    );
    let person = visual.update(|_, cx| {
        shell
            .read(cx)
            .core
            .store()
            .instance(&inst)
            .unwrap()
            .me_id()
            .unwrap()
    });
    let wanted = [
        (file_part, menu_target),
        (
            author_part,
            Target::Member(inst.clone(), Some(space), person),
        ),
    ];
    for (part, wanted) in wanted {
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let at = visual
            .update(|_, cx| shell.read(cx).selection.bounds_of(part))
            .expect("drawn")
            .center();
        visual.simulate_mouse_move(at, None, Modifiers::none());
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        visual.simulate_mouse_down(at, right, Modifiers::none());
        visual.run_until_parked();
        visual.update(|_, cx| {
            assert_eq!(shell.read(cx).right_pressed.as_ref(), Some(&wanted));
        });
        visual.simulate_mouse_up(at, right, Modifiers::none());
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            shell.update(cx, |this, cx| {
                assert!(this.menu.is_some(), "the menu of {wanted:?} opened");
                this.menu = None;
                cx.notify();
            });
        });
        visual.run_until_parked();
    }

    // The speaker opens a slider, and going from one to the other never closes it.
    let sound = visual.update(|_, cx| {
        let core = shell.read(cx).core.clone();
        let store = core.store();
        let window = store
            .instance(&inst)
            .and_then(|s| s.window(channel))
            .unwrap();
        window.messages[2].attachments[0].id
    });
    let redraw = |visual: &mut VisualTestContext| {
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    };
    redraw(&mut visual);
    let speaker = visual
        .debug_bounds("media-speaker")
        .expect("the speaker is drawn")
        .center();
    visual.simulate_mouse_move(speaker, None, Modifiers::none());
    redraw(&mut visual);
    let hovered = |visual: &mut VisualTestContext| {
        visual.update(|_, cx| shell.read(cx).volume_hovered(&inst, sound))
    };
    assert!(hovered(&mut visual), "the speaker opens the slider");
    // Let the opening animation finish, so the slider has its full width.
    for _ in 0..20 {
        visual.executor().advance_clock(Duration::from_millis(15));
        redraw(&mut visual);
    }
    let track = visual
        .update(|_, cx| shell.read(cx).slider_bounds(&inst, sound))
        .expect("the slider is drawn");
    // Step by step across the border of slider and speaker.
    let (left, right) = (speaker.x.as_f32(), track.center().x.as_f32());
    for step in 0..=24 {
        let x = left + (right - left) * step as f32 / 24.;
        visual.simulate_mouse_move(point(px(x), speaker.y), None, Modifiers::none());
        redraw(&mut visual);
        assert!(hovered(&mut visual), "still open {step}/24 of the way");
    }
    // Dragging the slider sets the volume, and it stays open when the mouse wanders off it.
    let quarter = point(track.left() + track.size.width * 0.25, track.center().y);
    let left_button = gpui_kit::MouseButton::Left;
    let volume = |visual: &mut VisualTestContext| {
        visual.update(|_, cx| shell.read(cx).config.voice.media_volume)
    };
    visual.simulate_mouse_move(quarter, None, Modifiers::none());
    redraw(&mut visual);
    visual.simulate_mouse_down(quarter, left_button, Modifiers::none());
    redraw(&mut visual);
    let pressed = volume(&mut visual);
    assert!(
        (pressed - 0.25).abs() < 0.02,
        "pressed at a quarter: {pressed}, track {track:?}, at {quarter:?}"
    );
    let far = point(track.right() + px(200.), track.center().y + px(90.));
    visual.simulate_mouse_move(far, Some(left_button), Modifiers::none());
    redraw(&mut visual);
    assert!(
        (volume(&mut visual) - 1.).abs() < 0.001,
        "dragged past its end"
    );
    visual.simulate_mouse_up(far, left_button, Modifiers::none());
    redraw(&mut visual);

    // Over the card: Ctrl+wheel sets the volume and the plain wheel does not.
    let name = visual
        .update(|_, cx| {
            shell.read(cx).selection.bounds_of(crate::selection::Part {
                message: visual_message(&shell, cx, &inst, channel, 2),
                index: crate::selection::FILES,
            })
        })
        .expect("the card is drawn")
        .center();
    visual.simulate_mouse_move(name, None, Modifiers::none());
    redraw(&mut visual);
    let before = volume(&mut visual);
    visual.simulate_event(ScrollEvent {
        position: name,
        delta: ScrollDelta::Lines(point(0.0, -1.0)),
        modifiers: Modifiers::control(),
        ..Default::default()
    });
    redraw(&mut visual);
    let after = volume(&mut visual);
    assert!((before - after - 0.05).abs() < 0.001, "{before} to {after}");
    visual.simulate_event(ScrollEvent {
        position: name,
        delta: ScrollDelta::Lines(point(0.0, -1.0)),
        ..Default::default()
    });
    redraw(&mut visual);
    assert_eq!(volume(&mut visual), after, "the plain wheel scrolls");

    // The Encryption tab loads the rules and draws them.
    visual.update(|window, cx| {
        shell.update(cx, |this, cx| {
            this.open_space_settings(
                inst.clone(),
                space,
                crate::space_settings::Tab::Encryption,
                window,
                cx,
            );
        });
    });
    for _ in 0..200 {
        visual.run_until_parked();
        let loaded = visual.update(|_, cx| {
            let this = shell.read(cx);
            this.space_pages
                .get(&(inst.clone(), space))
                .is_some_and(|page| matches!(page.keys.policy, crate::results::Remote::Ready(_)))
        });
        if loaded {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    visual.update(|window, cx| {
        let page = shell
            .read(cx)
            .space_pages
            .get(&(inst.clone(), space))
            .unwrap();
        assert!(matches!(page.keys.policy, crate::results::Remote::Ready(_)));
        window.refresh();
        window.draw(cx).clear(cx);
        shell.update(cx, |this, cx| this.close_space_settings(cx));
    });
    visual.run_until_parked();

    // Each kind of menu opens and draws, submenus included.
    let targets = [
        Target::Space(inst.clone(), space),
        Target::Channel(inst.clone(), channel),
        Target::Me(Some(inst.clone())),
        Target::Me(None),
        Target::Instances,
        Target::ChannelArea(inst.clone(), space),
    ];
    for target in targets {
        visual.update(|window, cx| {
            shell.update(cx, |this, cx| {
                this.open_menu(target, position, window, cx);
            });
            window.refresh();
            window.draw(cx).clear(cx);
            shell.update(cx, |this, cx| {
                assert!(this.menu.is_some());
                this.menu = None;
                cx.notify();
            });
        });
        visual.run_until_parked();
    }
}
