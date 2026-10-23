use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::backend::debloat::{self, DebloatFeatureDescriptor, WindowsCustomConfig};
use crate::backend::drive::{self, RemovableDrive};
use crate::backend::writer::{
    self, FileSystemType, FlashJobConfig, FlashMode, FlashProgressEvent, IsoInspection,
    PartitionScheme,
};

use super::APP_ID;

const MODE_LABELS: [&str; 2] = [
    "Mode DD — Écriture brute par blocs (Linux Hybride / IMG)",
    "Mode Windows UEFI — Partitionnement + extraction + debloat",
];
const SCHEME_LABELS: [&str; 2] = ["GPT (UEFI moderne)", "MBR (BIOS / UEFI-CSM)"];
const FS_LABELS: [&str; 2] = [
    "FAT32 (standard UEFI + découpage wimlib)",
    "NTFS (fichiers > 4 Gio natifs)",
];

#[derive(Clone, Copy)]
enum Preset {
    Recommended,
    All,
    Minimal,
    None,
}

#[derive(Clone, Copy)]
enum LogLevel {
    Info,
    Success,
    Error,
}

struct State {
    drives: Vec<RemovableDrive>,
    selected_drive: Option<RemovableDrive>,
    iso: Option<IsoInspection>,
    catalog: Vec<DebloatFeatureDescriptor>,
    enabled: HashSet<String>,
    is_flashing: bool,
    cancel_flag: Arc<AtomicBool>,
}

impl State {
    fn new() -> Self {
        let catalog = debloat::get_debloat_catalog();
        let enabled = catalog
            .iter()
            .filter(|feature| feature.default_enabled)
            .map(|feature| feature.id.clone())
            .collect();
        Self {
            drives: Vec::new(),
            selected_drive: None,
            iso: None,
            catalog,
            enabled,
            is_flashing: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

enum FlashMessage {
    Progress(FlashProgressEvent),
    Finished(Result<(), String>),
}

#[derive(Clone)]
struct Ui {
    window: adw::ApplicationWindow,
    toast: adw::ToastOverlay,
    refresh_button: gtk::Button,
    browse_button: gtk::Button,
    drive_combo: adw::ComboRow,
    drive_placeholder: gtk::Label,
    drive_device_row: adw::ActionRow,
    drive_model_row: adw::ActionRow,
    drive_size_row: adw::ActionRow,
    drive_mount_row: adw::ActionRow,
    iso_path_row: adw::EntryRow,
    iso_placeholder: gtk::Label,
    iso_kind_row: adw::ActionRow,
    iso_file_row: adw::ActionRow,
    iso_size_row: adw::ActionRow,
    iso_boot_row: adw::ActionRow,
    iso_wim_row: adw::ActionRow,
    mode_combo: adw::ComboRow,
    scheme_combo: adw::ComboRow,
    fs_combo: adw::ComboRow,
    label_row: adw::EntryRow,
    wim_warning: gtk::Label,
    progress_stage: gtk::Label,
    progress_eta: gtk::Label,
    progress_bar: gtk::ProgressBar,
    progress_speed: gtk::Label,
    progress_percent: gtk::Label,
    progress_bytes: gtk::Label,
    progress_status: gtk::Label,
    log_buffer: gtk::TextBuffer,
    log_view: gtk::TextView,
    copy_logs_button: gtk::Button,
    clear_logs_button: gtk::Button,
    start_button: gtk::Button,
    cancel_button: gtk::Button,
    studio_pane: gtk::ScrolledWindow,
    studio_hint: gtk::Label,
    studio_switch: adw::SwitchRow,
    studio_body: gtk::Box,
    force_local_switch: adw::SwitchRow,
    create_local_switch: adw::SwitchRow,
    auto_logon_switch: adw::SwitchRow,
    skip_privacy_switch: adw::SwitchRow,
    french_locale_switch: adw::SwitchRow,
    username_row: adw::EntryRow,
    password_row: adw::PasswordEntryRow,
    debloat_counter: gtk::Label,
    debloat_switches: Rc<RefCell<Vec<(String, adw::SwitchRow)>>>,
    preset_buttons: Vec<(Preset, gtk::Button)>,
    preview_button: gtk::Button,
    about_button: gtk::Button,
}

pub fn present(app: &adw::Application, initial_iso: Option<String>) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Noos Flasher")
        .default_width(1140)
        .default_height(860)
        .build();
    window.set_size_request(940, 680);

    let state = Rc::new(RefCell::new(State::new()));
    let ui = Rc::new(Ui::build(&window, &state));

    wire_events(ui.clone(), state.clone());

    let initial_iso_ui = ui.clone();
    let initial_iso_state = state.clone();
    initial_load(ui, state);
    window.present();

    if let Some(path) = initial_iso {
        inspect_iso(&initial_iso_ui, &initial_iso_state, path);
    }
}

impl Ui {
    fn build(window: &adw::ApplicationWindow, state: &Rc<RefCell<State>>) -> Self {
        let toast = adw::ToastOverlay::new();
        let banner = adw::Banner::new(
            "Attention : toutes les données de la clé USB cible seront définitivement écrasées.",
        );
        banner.set_revealed(true);

        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new("Noos Flasher", "Créateur de clés USB bootables");
        header.set_title_widget(Some(&title));

        let about_button = gtk::Button::from_icon_name("help-about-symbolic");
        about_button.set_tooltip_text(Some("À propos de Noos Flasher"));
        about_button.add_css_class("flat");

        let start_button = gtk::Button::with_label("Démarrer");
        start_button.add_css_class("suggested-action");
        start_button.set_tooltip_text(Some("Démarrer le flashage de la clé USB"));
        start_button.set_sensitive(false);
        let cancel_button = gtk::Button::with_label("Annuler");
        cancel_button.add_css_class("destructive-action");
        cancel_button.set_tooltip_text(Some("Annuler l'opération en cours"));
        cancel_button.set_visible(false);

        header.pack_end(&about_button);
        header.pack_end(&start_button);
        header.pack_end(&cancel_button);

        // ---------------------------------------------------------------
        // Étape 1 — périphérique USB
        // ---------------------------------------------------------------
        let drive_combo = adw::ComboRow::builder()
            .title("Clé USB cible")
            .use_markup(false)
            .build();
        let drive_combo_model = gtk::StringList::new(&["Recherche des périphériques amovibles…"]);
        drive_combo.set_model(Some(&drive_combo_model));

        let refresh_button = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_button.set_tooltip_text(Some("Actualiser la liste des périphériques USB"));
        refresh_button.add_css_class("flat");

        let drive_placeholder = gtk::Label::new(Some(
            "Aucun périphérique USB sélectionné. Branchez une clé puis actualisez.",
        ));
        drive_placeholder.set_xalign(0.0);
        drive_placeholder.set_wrap(true);
        drive_placeholder.add_css_class("dim");

        let drive_device_row = detail_row("Périphérique");
        let drive_model_row = detail_row("Modèle / Vendeur");
        let drive_size_row = detail_row("Capacité totale");
        let drive_mount_row = detail_row("Points de montage");

        let drive_group = adw::PreferencesGroup::builder()
            .title("1. Périphérique USB")
            .description(
                "Les disques internes, NVMe et partitions système sont exclus par sécurité",
            )
            .build();
        drive_group.set_header_suffix(Some(&refresh_button));
        drive_group.add(&drive_combo);
        drive_group.add(&drive_placeholder);
        drive_group.add(&drive_device_row);
        drive_group.add(&drive_model_row);
        drive_group.add(&drive_size_row);
        drive_group.add(&drive_mount_row);

        // ---------------------------------------------------------------
        // Étape 2 — image ISO / IMG
        // ---------------------------------------------------------------
        let browse_button = gtk::Button::with_label("Parcourir…");
        browse_button.set_tooltip_text(Some("Sélectionner un fichier .iso ou .img"));
        browse_button.add_css_class("flat");

        let iso_path_row = adw::EntryRow::builder()
            .title("Chemin du fichier image")
            .use_markup(false)
            .build();
        iso_path_row.set_editable(false);

        let iso_placeholder = gtk::Label::new(Some(
            "Sélectionnez un fichier .iso ou .img pour analyser sa structure d'amorçage.",
        ));
        iso_placeholder.set_xalign(0.0);
        iso_placeholder.set_wrap(true);
        iso_placeholder.add_css_class("dim");

        let iso_kind_row = detail_row("Type détecté");
        let iso_file_row = detail_row("Fichier");
        let iso_size_row = detail_row("Taille & label");
        let iso_boot_row = detail_row("Amorçage détecté");
        let iso_wim_row = detail_row("Fichier install.wim");

        let iso_group = adw::PreferencesGroup::builder()
            .title("2. Image disque (.iso / .img)")
            .description("Détection automatique Linux hybride, IMG brute ou Windows UEFI")
            .build();
        iso_group.set_header_suffix(Some(&browse_button));
        iso_group.add(&iso_path_row);
        iso_group.add(&iso_placeholder);
        iso_group.add(&iso_kind_row);
        iso_group.add(&iso_file_row);
        iso_group.add(&iso_size_row);
        iso_group.add(&iso_boot_row);
        iso_group.add(&iso_wim_row);

        // ---------------------------------------------------------------
        // Étape 3 — formatage et schéma d'amorçage
        // ---------------------------------------------------------------
        let mode_combo = adw::ComboRow::builder()
            .title("Méthode de flashage")
            .model(&gtk::StringList::new(&MODE_LABELS))
            .use_markup(false)
            .build();
        mode_combo.set_selected(0);

        let scheme_combo = adw::ComboRow::builder()
            .title("Schéma de partition")
            .model(&gtk::StringList::new(&SCHEME_LABELS))
            .use_markup(false)
            .build();
        scheme_combo.set_selected(0);

        let fs_combo = adw::ComboRow::builder()
            .title("Système de fichiers")
            .model(&gtk::StringList::new(&FS_LABELS))
            .use_markup(false)
            .build();
        fs_combo.set_selected(0);

        let label_row = adw::EntryRow::builder()
            .title("Nom du volume (label)")
            .text("NOOS_USB")
            .use_markup(false)
            .build();

        let wim_warning = gtk::Label::new(None);
        wim_warning.set_xalign(0.0);
        wim_warning.set_wrap(true);
        wim_warning.add_css_class("warning-text");
        wim_warning.set_visible(false);

        let format_group = adw::PreferencesGroup::builder()
            .title(escape_markup("3. Formatage & schéma d'amorçage"))
            .description("Ajusté automatiquement selon le type d'image détecté")
            .build();
        format_group.add(&mode_combo);
        format_group.add(&scheme_combo);
        format_group.add(&fs_combo);
        format_group.add(&label_row);
        format_group.add(&wim_warning);

        // ---------------------------------------------------------------
        // Progression et journal
        // ---------------------------------------------------------------
        let progress_stage = gtk::Label::new(Some("Prêt à flasher"));
        progress_stage.set_xalign(0.0);
        progress_stage.add_css_class("stage-title");
        let progress_eta = gtk::Label::new(None);
        progress_eta.set_xalign(0.0);
        progress_eta.add_css_class("dim");

        let progress_speed = gtk::Label::new(Some("0.0 Mo/s"));
        progress_speed.add_css_class("pill");
        progress_speed.add_css_class("neutral");
        let progress_percent = gtk::Label::new(Some("0.0 %"));
        progress_percent.add_css_class("pill");
        progress_percent.add_css_class("accent");

        let stage_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        stage_box.set_hexpand(true);
        stage_box.append(&progress_stage);
        stage_box.append(&progress_eta);

        let metrics_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        metrics_box.set_valign(gtk::Align::Center);
        metrics_box.append(&progress_speed);
        metrics_box.append(&progress_percent);

        let progress_header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        progress_header.append(&stage_box);
        progress_header.append(&metrics_box);

        let progress_bar = gtk::ProgressBar::new();
        progress_bar.set_show_text(false);
        progress_bar.set_fraction(0.0);
        progress_bar.set_hexpand(true);

        let progress_bytes = gtk::Label::new(Some("0.0 Mio / 0.0 Mio"));
        progress_bytes.set_xalign(0.0);
        progress_bytes.add_css_class("dim");
        let progress_status = gtk::Label::new(Some("En attente"));
        progress_status.set_xalign(1.0);
        progress_status.add_css_class("status-idle");

        let progress_subinfo = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        progress_bytes.set_hexpand(true);
        progress_subinfo.append(&progress_bytes);
        progress_subinfo.append(&progress_status);

        let log_buffer = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
        log_buffer.create_tag(Some("info"), &[]);
        log_buffer.create_tag(Some("success"), &[("foreground", &"#2ec27e")]);
        log_buffer.create_tag(Some("error"), &[("foreground", &"#e01b24")]);

        let log_view = gtk::TextView::builder()
            .buffer(&log_buffer)
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .build();
        log_view.add_css_class("log-view");
        log_view.set_top_margin(6);
        log_view.set_bottom_margin(6);
        log_view.set_left_margin(6);
        log_view.set_right_margin(6);

        let log_scroll = gtk::ScrolledWindow::builder()
            .child(&log_view)
            .min_content_height(150)
            .max_content_height(240)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let copy_logs_button = gtk::Button::with_label("Copier");
        copy_logs_button.add_css_class("flat");
        copy_logs_button.add_css_class("preset-button");
        let clear_logs_button = gtk::Button::with_label("Effacer");
        clear_logs_button.add_css_class("flat");
        clear_logs_button.add_css_class("preset-button");

        let log_toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let log_title = gtk::Label::new(Some("Journal d'opérations"));
        log_title.set_xalign(0.0);
        log_title.set_hexpand(true);
        log_title.add_css_class("dim");
        log_toolbar.append(&log_title);
        log_toolbar.append(&copy_logs_button);
        log_toolbar.append(&clear_logs_button);

        let log_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        log_box.append(&log_toolbar);
        log_box.append(&log_scroll);

        let log_expander = gtk::Expander::builder()
            .label("Journal d'opérations bas niveau")
            .expanded(true)
            .child(&log_box)
            .build();

        let progress_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        progress_box.append(&progress_header);
        progress_box.append(&progress_bar);
        progress_box.append(&progress_subinfo);
        progress_box.append(&log_expander);

        let progress_group = adw::PreferencesGroup::builder()
            .title(escape_markup("Progression & journal"))
            .build();
        progress_group.add(&progress_box);

        let left_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        left_page.set_margin_top(16);
        left_page.set_margin_bottom(16);
        left_page.set_margin_start(16);
        left_page.set_margin_end(10);
        left_page.append(&drive_group);
        left_page.append(&iso_group);
        left_page.append(&format_group);
        left_page.append(&progress_group);

        let left_scroll = gtk::ScrolledWindow::builder()
            .child(&left_page)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        // ---------------------------------------------------------------
        // Studio Windows / debloat
        // ---------------------------------------------------------------
        let studio_hint = gtk::Label::new(Some(
            "Le studio Windows et le debloat sont disponibles lorsque le mode Windows UEFI est sélectionné.",
        ));
        studio_hint.set_xalign(0.0);
        studio_hint.set_wrap(true);
        studio_hint.add_css_class("dim");

        let studio_switch = adw::SwitchRow::builder()
            .title("Activer l'installation personnalisée & le debloat")
            .subtitle("Génère autounattend.xml et les scripts PowerShell sur la clé USB")
            .active(true)
            .use_markup(false)
            .build();

        let preview_button = gtk::Button::with_label("Aperçu scripts");
        preview_button.add_css_class("flat");
        preview_button.add_css_class("preset-button");

        let studio_group = adw::PreferencesGroup::builder()
            .title("4. Personnalisation Windows")
            .build();
        studio_group.set_header_suffix(Some(&preview_button));
        studio_group.add(&studio_switch);

        let force_local_switch = adw::SwitchRow::builder()
            .title("Forcer un compte local Windows (BypassNRO)")
            .subtitle("Masque l'obligation de compte Microsoft en ligne pendant l'OOBE")
            .active(true)
            .build();
        let create_local_switch = adw::SwitchRow::builder()
            .title("Créer le compte administrateur local")
            .subtitle("Pré-configure le compte sans saisie manuelle pendant l'OOBE")
            .active(true)
            .build();
        let auto_logon_switch = adw::SwitchRow::builder()
            .title("Connexion automatique au premier démarrage")
            .active(true)
            .build();
        let skip_privacy_switch = adw::SwitchRow::builder()
            .title("Ignorer les questions de confidentialité et l'EULA")
            .active(true)
            .build();
        let french_locale_switch = adw::SwitchRow::builder()
            .title("Langue et clavier français (fr-FR)")
            .active(true)
            .build();

        let username_row = adw::EntryRow::builder()
            .title("Nom d'utilisateur Windows")
            .text("Utilisateur")
            .build();

        let password_row = adw::PasswordEntryRow::builder()
            .title("Mot de passe (vide = aucun)")
            .build();

        let account_group = adw::PreferencesGroup::builder()
            .title(escape_markup("Compte utilisateur local & OOBE"))
            .description("Inspiré des options de personnalisation de Rufus")
            .build();
        account_group.add(&force_local_switch);
        account_group.add(&create_local_switch);
        account_group.add(&username_row);
        account_group.add(&password_row);
        account_group.add(&auto_logon_switch);
        account_group.add(&skip_privacy_switch);
        account_group.add(&french_locale_switch);

        let debloat_counter = gtk::Label::new(None);
        debloat_counter.add_css_class("pill");
        debloat_counter.add_css_class("accent");
        debloat_counter.set_xalign(0.0);
        debloat_counter.set_hexpand(true);

        let preset_buttons = [
            ("Recommandé", Preset::Recommended),
            ("Tout cocher", Preset::All),
            ("Minimal (TPM)", Preset::Minimal),
            ("Aucun", Preset::None),
        ]
        .into_iter()
        .map(|(label, preset)| {
            let button = gtk::Button::with_label(label);
            button.add_css_class("flat");
            button.add_css_class("preset-button");
            (preset, button)
        })
        .collect::<Vec<_>>();

        let preset_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        preset_box.set_halign(gtk::Align::End);
        for (_, button) in &preset_buttons {
            preset_box.append(button);
        }

        let debloat_toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        debloat_toolbar.set_margin_top(4);
        debloat_toolbar.set_margin_bottom(4);
        debloat_toolbar.append(&debloat_counter);
        debloat_toolbar.append(&preset_box);

        let debloat_switches: Rc<RefCell<Vec<(String, adw::SwitchRow)>>> =
            Rc::new(RefCell::new(Vec::new()));

        let mut debloat_groups: Vec<adw::PreferencesGroup> = Vec::new();
        let mut current_category: Option<String> = None;
        let mut current_group: Option<adw::PreferencesGroup> = None;

        for feature in &state.borrow().catalog {
            if current_category.as_deref() != Some(feature.category.as_str()) {
                if let Some(group) = current_group.take() {
                    debloat_groups.push(group);
                }
                let group = adw::PreferencesGroup::builder()
                    .title(escape_markup(&feature.category))
                    .build();
                current_category = Some(feature.category.clone());
                current_group = Some(group);
            }

            let is_active = state.borrow().enabled.contains(&feature.id);
            let row = adw::SwitchRow::builder()
                .title(&feature.title)
                .subtitle(&feature.description)
                .active(is_active)
                .use_markup(false)
                .build();
            row.set_subtitle_lines(3);

            let badge = gtk::Label::new(Some(&feature.badge));
            badge.add_css_class("pill");
            badge.add_css_class("neutral");
            badge.add_css_class("small");
            row.add_suffix(&badge);

            if let Some(group) = current_group.as_ref() {
                group.add(&row);
            }
            debloat_switches
                .borrow_mut()
                .push((feature.id.clone(), row));
        }
        if let Some(group) = current_group.take() {
            debloat_groups.push(group);
        }

        let studio_body = gtk::Box::new(gtk::Orientation::Vertical, 12);
        studio_body.append(&account_group);
        studio_body.append(&debloat_toolbar);
        for group in debloat_groups {
            studio_body.append(&group);
        }

        let right_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        right_page.set_margin_top(16);
        right_page.set_margin_bottom(16);
        right_page.set_margin_start(10);
        right_page.set_margin_end(16);
        right_page.append(&studio_hint);
        right_page.append(&studio_group);
        right_page.append(&studio_body);

        let right_scroll = gtk::ScrolledWindow::builder()
            .child(&right_page)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_wide_handle(true);
        paned.set_resize_start_child(true);
        paned.set_resize_end_child(true);
        paned.set_shrink_start_child(true);
        paned.set_shrink_end_child(true);
        paned.set_position(660);
        paned.set_start_child(Some(&left_scroll));
        paned.set_end_child(Some(&right_scroll));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&banner);
        content.append(&paned);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&content));

        toast.set_child(Some(&toolbar_view));
        window.set_content(Some(&toast));

        Self {
            window: window.clone(),
            toast,
            refresh_button,
            browse_button,
            drive_combo,
            drive_placeholder,
            drive_device_row,
            drive_model_row,
            drive_size_row,
            drive_mount_row,
            iso_path_row,
            iso_placeholder,
            iso_kind_row,
            iso_file_row,
            iso_size_row,
            iso_boot_row,
            iso_wim_row,
            mode_combo,
            scheme_combo,
            fs_combo,
            label_row,
            wim_warning,
            progress_stage,
            progress_eta,
            progress_bar,
            progress_speed,
            progress_percent,
            progress_bytes,
            progress_status,
            log_buffer,
            log_view,
            copy_logs_button,
            clear_logs_button,
            start_button,
            cancel_button,
            studio_pane: right_scroll,
            studio_hint,
            studio_switch,
            studio_body,
            force_local_switch,
            create_local_switch,
            auto_logon_switch,
            skip_privacy_switch,
            french_locale_switch,
            username_row,
            password_row,
            debloat_counter,
            debloat_switches,
            preset_buttons,
            preview_button,
            about_button,
        }
    }
}

fn detail_row(title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle("")
        .use_markup(false)
        .build();
    row.set_subtitle_lines(2);
    row.set_visible(false);
    row
}

fn wire_events(ui: Rc<Ui>, state: Rc<RefCell<State>>) {
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.refresh_button
            .connect_clicked(move |_| refresh_drives(&ui2, &state2));
    }
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.browse_button
            .connect_clicked(move |_| choose_iso(&ui2, &state2));
    }
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.drive_combo.connect_selected_notify(move |_| {
            let idx = ui2.drive_combo.selected() as usize;
            let selected = {
                let st = state2.borrow();
                st.drives.get(idx).cloned()
            };
            state2.borrow_mut().selected_drive = selected;
            let st = state2.borrow();
            render_drive_details(&ui2, &st);
            update_ready(&ui2, &st);
        });
    }
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.mode_combo.connect_selected_notify(move |_| {
            sync_mode(&ui2, &state2.borrow());
            update_ready(&ui2, &state2.borrow());
        });
    }
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.scheme_combo.connect_selected_notify(move |_| {
            sync_mode(&ui2, &state2.borrow());
        });
    }
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.fs_combo.connect_selected_notify(move |_| {
            sync_mode(&ui2, &state2.borrow());
            update_ready(&ui2, &state2.borrow());
        });
    }
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.studio_switch.connect_active_notify(move |_| {
            sync_mode(&ui2, &state2.borrow());
        });
    }
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.create_local_switch.connect_active_notify(move |_| {
            sync_mode(&ui2, &state2.borrow());
        });
    }
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.start_button
            .connect_clicked(move |_| show_confirmation(&ui2, &state2));
    }
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.cancel_button
            .connect_clicked(move |_| request_cancel(&ui2, &state2));
    }
    {
        let ui2 = ui.clone();
        let state2 = state.clone();
        ui.preview_button
            .connect_clicked(move |_| show_preview(&ui2, &state2));
    }
    for (preset, button) in ui.preset_buttons.clone() {
        let ui2 = ui.clone();
        let state2 = state.clone();
        button.connect_clicked(move |_| apply_preset(&ui2, &state2, preset));
    }
    for (id, switch_row) in ui.debloat_switches.borrow().iter() {
        let id = id.clone();
        let ui2 = ui.clone();
        let state2 = state.clone();
        switch_row.connect_active_notify(move |row| {
            let mut st = state2.borrow_mut();
            if row.is_active() {
                st.enabled.insert(id.clone());
            } else {
                st.enabled.remove(&id);
            }
            drop(st);
            update_debloat_counter(&ui2, &state2.borrow());
        });
    }
    {
        let ui2 = ui.clone();
        ui.copy_logs_button.connect_clicked(move |_| {
            let text = ui2
                .log_buffer
                .text(
                    &ui2.log_buffer.start_iter(),
                    &ui2.log_buffer.end_iter(),
                    false,
                )
                .to_string();
            ui2.window.clipboard().set_text(&text);
            ui2.toast.add_toast(adw::Toast::new("Journal copié."));
        });
    }
    {
        let ui2 = ui.clone();
        ui.clear_logs_button.connect_clicked(move |_| {
            ui2.log_buffer.set_text("");
        });
    }
    {
        let ui2 = ui.clone();
        ui.about_button.connect_clicked(move |_| {
            let about = adw::AboutWindow::builder()
                .application_name("Noos Flasher")
                .application_icon(APP_ID)
                .developer_name("Chomiam")
                .version(env!("CARGO_PKG_VERSION"))
                .website("https://github.com/Chomiam/noos-flasher")
                .issue_url("https://github.com/Chomiam/noos-flasher/issues")
                .license_type(gtk::License::MitX11)
                .build();
            about.set_transient_for(Some(&ui2.window));
            about.set_modal(true);
            about.present();
        });
    }
    {
        let state2 = state.clone();
        let toast = ui.toast.clone();
        ui.window.connect_close_request(move |_| {
            if state2.borrow().is_flashing {
                toast.add_toast(adw::Toast::new(
                    "Une opération est en cours. Annulez-la avant de quitter.",
                ));
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
}

fn initial_load(ui: Rc<Ui>, state: Rc<RefCell<State>>) {
    append_log(
        &ui,
        "Noos Flasher initialisé — interface GTK4 / Libadwaita.",
        LogLevel::Info,
    );
    append_log(&ui, "Recherche des périphériques USB…", LogLevel::Info);
    update_debloat_counter(&ui, &state.borrow());
    sync_mode(&ui, &state.borrow());
    refresh_drives(&ui, &state);
    render_iso_details(&ui, None, None);
    render_drive_details(&ui, &state.borrow());
    update_ready(&ui, &state.borrow());
}

fn refresh_drives(ui: &Rc<Ui>, state: &Rc<RefCell<State>>) {
    if state.borrow().is_flashing {
        return;
    }
    ui.refresh_button.set_sensitive(false);
    let ui2 = ui.clone();
    let state2 = state.clone();
    spawn_background(drive::list_removable_drives, move |result| {
        ui2.refresh_button.set_sensitive(true);
        match result {
            Ok(drives) => {
                let previous_path = state2
                    .borrow()
                    .selected_drive
                    .as_ref()
                    .map(|drive| drive.path.clone());
                let selected_index = previous_path
                    .and_then(|path| drives.iter().position(|drive| drive.path == path))
                    .unwrap_or(0);

                {
                    let mut st = state2.borrow_mut();
                    st.drives = drives;
                    st.selected_drive = st.drives.get(selected_index).cloned();
                }

                let (labels, is_empty, is_flashing, drive_count) = {
                    let st = state2.borrow();
                    let labels = st
                        .drives
                        .iter()
                        .map(|drive| {
                            format!(
                                "{} — {} {} ({})",
                                drive.path, drive.vendor, drive.model, drive.size_formatted
                            )
                        })
                        .collect::<Vec<_>>();
                    (
                        labels,
                        st.drives.is_empty(),
                        st.is_flashing,
                        st.drives.len(),
                    )
                };

                if is_empty {
                    let empty = gtk::StringList::new(&["Aucun périphérique USB amovible détecté"]);
                    ui2.drive_combo.set_model(Some(&empty));
                    ui2.drive_combo.set_selected(0);
                    ui2.drive_combo.set_sensitive(false);
                } else {
                    let labels_ref = labels.iter().map(String::as_str).collect::<Vec<_>>();
                    let model = gtk::StringList::new(&labels_ref);
                    ui2.drive_combo.set_model(Some(&model));
                    ui2.drive_combo.set_selected(selected_index as u32);
                    ui2.drive_combo.set_sensitive(!is_flashing);
                }

                let st = state2.borrow();
                render_drive_details(&ui2, &st);
                update_ready(&ui2, &st);
                drop(st);
                append_log(
                    &ui2,
                    &format!("{drive_count} périphérique(s) USB amovible(s) détecté(s)."),
                    LogLevel::Info,
                );
            }
            Err(err) => {
                append_log(
                    &ui2,
                    &format!("Erreur lors de l'énumération des disques : {err}"),
                    LogLevel::Error,
                );
                ui2.toast
                    .add_toast(adw::Toast::new("Impossible d'énumérer les disques USB."));
            }
        }
    });
}

fn render_drive_details(ui: &Ui, state: &State) {
    let Some(drive) = state.selected_drive.as_ref() else {
        for row in [
            &ui.drive_device_row,
            &ui.drive_model_row,
            &ui.drive_size_row,
            &ui.drive_mount_row,
        ] {
            row.set_visible(false);
        }
        ui.drive_placeholder.set_visible(true);
        return;
    };

    ui.drive_placeholder.set_visible(false);
    ui.drive_device_row.set_visible(true);
    ui.drive_model_row.set_visible(true);
    ui.drive_size_row.set_visible(true);
    ui.drive_mount_row.set_visible(true);

    ui.drive_device_row
        .set_subtitle(&format!("{} ({})", drive.path, drive.transport));
    ui.drive_model_row.set_subtitle(&format!(
        "{} {} {}",
        drive.vendor,
        drive.model,
        if drive.read_only {
            "— lecture seule"
        } else {
            ""
        }
    ));
    ui.drive_size_row.set_subtitle(&drive.size_formatted);
    ui.drive_mount_row.set_subtitle(&format!(
        "{} — {} partition(s)",
        drive.mountpoint_display,
        drive.partitions.len()
    ));
}

fn choose_iso(ui: &Rc<Ui>, state: &Rc<RefCell<State>>) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Images disque (.iso, .img)"));
    filter.add_suffix("iso");
    filter.add_suffix("img");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);

    let dialog = gtk::FileDialog::builder()
        .title("Sélectionner une image ISO ou IMG bootable")
        .filters(&filters)
        .modal(true)
        .build();

    let ui2 = ui.clone();
    let state2 = state.clone();
    dialog.open(Some(&ui.window), None::<&gio::Cancellable>, move |result| {
        if let Ok(file) = result {
            if let Some(path) = file.path() {
                inspect_iso(&ui2, &state2, path.to_string_lossy().to_string());
            }
        }
    });
}

fn inspect_iso(ui: &Rc<Ui>, state: &Rc<RefCell<State>>, path: String) {
    ui.iso_path_row.set_text(&path);
    append_log(
        ui,
        &format!("Inspection de l'image : {path}…"),
        LogLevel::Info,
    );
    ui.browse_button.set_sensitive(false);

    let ui2 = ui.clone();
    let state2 = state.clone();
    spawn_background(
        move || writer::inspect_iso_file(&path),
        move |result| {
            ui2.browse_button.set_sensitive(true);
            match result {
                Ok(info) => {
                    state2.borrow_mut().iso = Some(info.clone());
                    ui2.iso_path_row.set_text(&info.path);
                    ui2.mode_combo
                        .set_selected(mode_index(info.recommended_mode));
                    ui2.scheme_combo
                        .set_selected(scheme_index(info.recommended_scheme));
                    ui2.fs_combo.set_selected(fs_index(info.recommended_fs));
                    ui2.label_row.set_text(&info.volume_label);
                    render_iso_details(&ui2, Some(&info), None);
                    append_log(
                        &ui2,
                        &format!(
                            "Image détectée : {} ({}).",
                            info.kind_label, info.size_formatted
                        ),
                        LogLevel::Success,
                    );
                    sync_mode(&ui2, &state2.borrow());
                    update_ready(&ui2, &state2.borrow());
                }
                Err(err) => {
                    state2.borrow_mut().iso = None;
                    render_iso_details(&ui2, None, Some(&err.to_string()));
                    append_log(&ui2, &format!("Image invalide : {err}"), LogLevel::Error);
                    sync_mode(&ui2, &state2.borrow());
                    update_ready(&ui2, &state2.borrow());
                }
            }
        },
    );
}

fn render_iso_details(ui: &Ui, info: Option<&IsoInspection>, error: Option<&str>) {
    let Some(info) = info else {
        for row in [
            &ui.iso_kind_row,
            &ui.iso_file_row,
            &ui.iso_size_row,
            &ui.iso_boot_row,
            &ui.iso_wim_row,
        ] {
            row.set_visible(false);
        }
        ui.iso_placeholder.set_visible(true);
        ui.iso_placeholder.set_text(error.unwrap_or(
            "Sélectionnez un fichier .iso ou .img pour analyser sa structure d'amorçage.",
        ));
        if error.is_some() {
            ui.iso_placeholder.add_css_class("error-text");
        } else {
            ui.iso_placeholder.remove_css_class("error-text");
        }
        return;
    };

    ui.iso_placeholder.set_visible(false);
    ui.iso_kind_row.set_visible(true);
    ui.iso_file_row.set_visible(true);
    ui.iso_size_row.set_visible(true);
    ui.iso_boot_row.set_visible(true);
    ui.iso_wim_row.set_visible(true);

    ui.iso_kind_row.set_title(&info.kind_label);
    ui.iso_kind_row.set_subtitle(&info.details);
    ui.iso_file_row.set_subtitle(&info.file_name);
    ui.iso_size_row.set_subtitle(&format!(
        "{} — label {}",
        info.size_formatted, info.volume_label
    ));

    let mut boot = Vec::new();
    if info.is_hybrid_mbr {
        boot.push("MBR hybride");
    }
    if info.has_gpt_header {
        boot.push("GPT");
    }
    if info.has_el_torito {
        boot.push("El Torito");
    }
    if info.is_udf {
        boot.push("UDF");
    }
    let boot_text = if boot.is_empty() {
        "Standard".to_string()
    } else {
        boot.join(" + ")
    };
    ui.iso_boot_row.set_subtitle(&boot_text);

    ui.iso_wim_row
        .set_subtitle(&match &info.install_wim_formatted {
            Some(size) if info.requires_wim_split_for_fat32 => {
                format!("{size} (> 4 Gio, découpage wimlib en FAT32)")
            }
            Some(size) => format!("{size} (compatible FAT32 et NTFS)"),
            None => "Non applicable".to_string(),
        });
}

fn sync_mode(ui: &Ui, state: &State) {
    let mode = mode_from_index(ui.mode_combo.selected());
    let is_windows = mode == FlashMode::WindowsUefi;
    let has_windows_iso = state
        .iso
        .as_ref()
        .map(|info| info.is_windows_iso)
        .unwrap_or(false);
    let flashing = state.is_flashing;
    let master = ui.studio_switch.is_active();
    let create_local = ui.create_local_switch.is_active();

    // Le volet de personnalisation Windows n'apparaît que pour une image
    // réellement détectée comme installateur Windows.
    ui.studio_pane.set_visible(has_windows_iso);

    ui.scheme_combo.set_sensitive(is_windows && !flashing);
    ui.fs_combo.set_sensitive(is_windows && !flashing);
    ui.label_row.set_sensitive(is_windows && !flashing);
    ui.studio_switch
        .set_sensitive(has_windows_iso && is_windows && !flashing);
    ui.studio_hint.set_visible(has_windows_iso && !is_windows);
    ui.studio_body
        .set_sensitive(has_windows_iso && is_windows && master && !flashing);
    ui.preview_button
        .set_sensitive(has_windows_iso && is_windows && !flashing);
    ui.username_row
        .set_sensitive(has_windows_iso && is_windows && master && create_local && !flashing);
    ui.password_row
        .set_sensitive(has_windows_iso && is_windows && master && create_local && !flashing);
    ui.auto_logon_switch
        .set_sensitive(has_windows_iso && is_windows && master && create_local && !flashing);

    let show_warning = if let Some(info) = state.iso.as_ref() {
        is_windows
            && ui.fs_combo.selected() == fs_index(FileSystemType::Fat32)
            && info.requires_wim_split_for_fat32
    } else {
        false
    };

    if show_warning {
        let info = state.iso.as_ref().expect("image inspectée");
        if info.wimlib_available {
            ui.wim_warning.set_text(
                "Note FAT32 : install.wim dépasse 4 Gio. Il sera découpé automatiquement en fichiers .swm (3800 Mio max) via wimlib-imagex.",
            );
            ui.wim_warning.remove_css_class("error-text");
            ui.wim_warning.add_css_class("warning-text");
        } else {
            ui.wim_warning.set_text(
                "Attention : install.wim dépasse 4 Gio et wimlib-imagex est absent. Sélectionnez NTFS ou installez wimtools.",
            );
            ui.wim_warning.remove_css_class("warning-text");
            ui.wim_warning.add_css_class("error-text");
        }
        ui.wim_warning.set_visible(true);
    } else {
        ui.wim_warning.set_visible(false);
    }
}

fn update_ready(ui: &Ui, state: &State) {
    let can_flash = !state.is_flashing && state.selected_drive.is_some() && state.iso.is_some();
    ui.start_button.set_sensitive(can_flash);
}

fn update_debloat_counter(ui: &Ui, state: &State) {
    ui.debloat_counter.set_text(&format!(
        "{} / {} actifs",
        state.enabled.len(),
        state.catalog.len()
    ));
}

fn apply_preset(ui: &Rc<Ui>, state: &Rc<RefCell<State>>, preset: Preset) {
    let catalog = state.borrow().catalog.clone();
    let new_enabled: HashSet<String> = match preset {
        Preset::Recommended => catalog
            .iter()
            .filter(|feature| feature.default_enabled)
            .map(|feature| feature.id.clone())
            .collect(),
        Preset::All => catalog.iter().map(|feature| feature.id.clone()).collect(),
        Preset::Minimal => [
            "bypass_win11_hw",
            "bypass_ram_storage",
            "disable_bitlocker_auto",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        Preset::None => HashSet::new(),
    };

    state.borrow_mut().enabled = new_enabled.clone();
    for (id, row) in ui.debloat_switches.borrow().iter() {
        row.set_active(new_enabled.contains(id));
    }
    update_debloat_counter(ui, &state.borrow());
}

fn build_windows_custom(ui: &Ui, state: &State) -> WindowsCustomConfig {
    WindowsCustomConfig {
        enabled: ui.studio_switch.is_active(),
        force_local_account: ui.force_local_switch.is_active(),
        create_local_user: ui.create_local_switch.is_active(),
        local_username: ui.username_row.text().trim().to_string(),
        local_password: ui.password_row.text().to_string(),
        auto_logon: ui.auto_logon_switch.is_active(),
        skip_oobe_privacy: ui.skip_privacy_switch.is_active(),
        set_french_locale: ui.french_locale_switch.is_active(),
        enabled_features: state.enabled.iter().cloned().collect(),
    }
}

fn show_confirmation(ui: &Rc<Ui>, state: &Rc<RefCell<State>>) {
    let (drive, iso) = {
        let st = state.borrow();
        (st.selected_drive.clone(), st.iso.clone())
    };
    let (Some(drive), Some(iso)) = (drive, iso) else {
        ui.toast
            .add_toast(adw::Toast::new("Sélectionnez une clé USB et une image."));
        return;
    };

    let mode = mode_from_index(ui.mode_combo.selected());
    let is_windows = mode == FlashMode::WindowsUefi;
    let scheme = scheme_from_index(ui.scheme_combo.selected());
    let fs = fs_from_index(ui.fs_combo.selected());
    let label = ui.label_row.text().to_string();
    let windows_custom = if is_windows {
        Some(build_windows_custom(ui, &state.borrow()))
    } else {
        None
    };

    let summary = gtk::Box::new(gtk::Orientation::Vertical, 6);
    summary.add_css_class("card-box");
    summary.set_size_request(460, -1);
    add_summary_row(
        &summary,
        "Cible matériel",
        &format!("{} — {} {}", drive.path, drive.vendor, drive.model),
    );
    add_summary_row(&summary, "Capacité de la clé", &drive.size_formatted);
    add_summary_row(
        &summary,
        "Image source",
        &format!("{} ({})", iso.file_name, iso.size_formatted),
    );
    let mode_description = if is_windows {
        format!(
            "Windows UEFI ({}/{})",
            scheme.as_str().to_uppercase(),
            fs.as_str().to_uppercase()
        )
    } else {
        "Mode DD (écriture brute bloc à bloc + sync)".to_string()
    };
    add_summary_row(&summary, "Mode d'écriture", &mode_description);
    if is_windows {
        add_summary_row(&summary, "Label du volume", &label);
        if let Some(ref custom) = windows_custom {
            let custom_description = if custom.enabled {
                format!(
                    "Actif — compte '{}' + {} patch(s)",
                    custom.local_username,
                    custom.enabled_features.len()
                )
            } else {
                "Désactivé (image d'origine non modifiée)".to_string()
            };
            add_summary_row(&summary, "Debloat & compte local", &custom_description);
        }
    }

    let dialog = adw::MessageDialog::new(
        Some(&ui.window),
        Some("Confirmer l'écrasement de la clé USB"),
        Some("Toutes les données existantes sur la clé cible seront définitivement détruites."),
    );
    dialog.add_response("cancel", "Annuler");
    dialog.add_response("flash", "Je confirme — Formater et flasher");
    dialog.set_response_appearance("flash", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.set_extra_child(Some(&summary));

    let ui2 = ui.clone();
    let state2 = state.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "flash" {
            start_flash(&ui2, &state2);
        }
    });
    dialog.present();
}

fn add_summary_row(parent: &gtk::Box, label: &str, value: &str) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("summary-row");
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_width_chars(18);
    label.add_css_class("summary-label");
    let value = gtk::Label::new(Some(value));
    value.set_xalign(0.0);
    value.set_hexpand(true);
    value.set_wrap(true);
    row.append(&label);
    row.append(&value);
    parent.append(&row);
}

fn start_flash(ui: &Rc<Ui>, state: &Rc<RefCell<State>>) {
    let (drive, iso, config) = {
        let st = state.borrow();
        if st.is_flashing {
            return;
        }
        let (Some(drive), Some(iso)) = (st.selected_drive.clone(), st.iso.clone()) else {
            return;
        };
        if iso.size_bytes > drive.size_bytes {
            drop(st);
            ui.toast.add_toast(adw::Toast::new(
                "L'image est plus volumineuse que la clé USB sélectionnée.",
            ));
            return;
        }
        let mode = mode_from_index(ui.mode_combo.selected());
        let windows_custom = if mode == FlashMode::WindowsUefi {
            Some(build_windows_custom(ui, &st))
        } else {
            None
        };
        let config = FlashJobConfig {
            device_path: drive.path.clone(),
            iso_path: iso.path.clone(),
            mode,
            partition_scheme: scheme_from_index(ui.scheme_combo.selected()),
            fs_type: fs_from_index(ui.fs_combo.selected()),
            volume_label: ui.label_row.text().to_string(),
            windows_custom,
        };
        (drive, iso, config)
    };

    {
        let mut st = state.borrow_mut();
        st.is_flashing = true;
        st.cancel_flag.store(false, Ordering::SeqCst);
    }

    ui.cancel_button.set_visible(true);
    ui.cancel_button.set_sensitive(true);
    ui.start_button.set_visible(false);
    ui.refresh_button.set_sensitive(false);
    ui.browse_button.set_sensitive(false);
    ui.drive_combo.set_sensitive(false);
    ui.mode_combo.set_sensitive(false);
    sync_mode(ui, &state.borrow());

    reset_progress(ui, iso.size_bytes);
    append_log(
        ui,
        &format!(
            "Démarrage du flashage sur {} (mode {}, image {}).",
            drive.path,
            config.mode.as_str(),
            iso.file_name
        ),
        LogLevel::Info,
    );

    let (tx, rx) = async_channel::unbounded::<FlashMessage>();
    let progress_tx = tx.clone();
    let cancel_flag = state.borrow().cancel_flag.clone();
    let job = config.clone();

    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                let _ = tx.send_blocking(FlashMessage::Finished(Err(format!(
                    "Impossible de démarrer le runtime de flashage : {err}"
                ))));
                return;
            }
        };

        let progress: writer::ProgressCallback = Arc::new(move |event| {
            let _ = progress_tx.send_blocking(FlashMessage::Progress(event));
        });

        let result = runtime.block_on(writer::execute_flash_with_pkexec(
            job,
            cancel_flag,
            progress,
        ));
        let _ = tx.send_blocking(FlashMessage::Finished(
            result.map_err(|err| err.to_string()),
        ));
    });

    let ui2 = ui.clone();
    let state2 = state.clone();
    glib::spawn_future_local(async move {
        while let Ok(message) = rx.recv().await {
            match message {
                FlashMessage::Progress(event) => apply_progress(&ui2, event),
                FlashMessage::Finished(result) => {
                    finish_flash(&ui2, &state2, result);
                    break;
                }
            }
        }
    });
}

fn reset_progress(ui: &Ui, total_bytes: u64) {
    ui.progress_stage.set_text("Initialisation du flashage…");
    ui.progress_eta.set_text("");
    ui.progress_speed.set_text("0.0 Mo/s");
    ui.progress_percent.set_text("0.0 %");
    ui.progress_bar.set_fraction(0.0);
    ui.progress_bar.remove_css_class("progress-bar-success");
    ui.progress_bar.remove_css_class("progress-bar-error");
    ui.progress_bytes.set_text(&format!(
        "{} / {}",
        format_progress_bytes(0),
        format_progress_bytes(total_bytes)
    ));
    set_status(ui, "En cours", "running");
}

fn apply_progress(ui: &Ui, event: FlashProgressEvent) {
    if !event.stage.is_empty() {
        ui.progress_stage.set_text(&event.stage);
    }
    ui.progress_eta.set_text(&format_eta(event.eta_seconds));
    ui.progress_speed
        .set_text(&format!("{:.1} Mo/s", event.speed_mbps));
    let percent = event.percent.clamp(0.0, 100.0);
    ui.progress_percent.set_text(&format!("{percent:.1} %"));
    ui.progress_bar.set_fraction(percent / 100.0);

    if event.total_bytes > 0 || event.bytes_processed > 0 {
        ui.progress_bytes.set_text(&format!(
            "{} / {}",
            format_progress_bytes(event.bytes_processed),
            format_progress_bytes(event.total_bytes)
        ));
    }

    if let Some(ref log) = event.log {
        let level = if event.error.is_some() {
            LogLevel::Error
        } else if event.completed {
            LogLevel::Success
        } else {
            LogLevel::Info
        };
        append_log(ui, log, level);
    }

    if event.completed {
        set_status(ui, "Terminé", "success");
        ui.progress_bar.add_css_class("progress-bar-success");
    } else if event.error.is_some() {
        set_status(ui, "Erreur", "error");
        ui.progress_bar.add_css_class("progress-bar-error");
    } else {
        set_status(ui, "En cours", "running");
    }
}

fn finish_flash(ui: &Rc<Ui>, state: &Rc<RefCell<State>>, result: Result<(), String>) {
    {
        let mut st = state.borrow_mut();
        st.is_flashing = false;
        st.cancel_flag.store(false, Ordering::SeqCst);
    }

    ui.cancel_button.set_visible(false);
    ui.start_button.set_visible(true);
    ui.refresh_button.set_sensitive(true);
    ui.browse_button.set_sensitive(true);
    ui.drive_combo.set_sensitive(true);
    ui.mode_combo.set_sensitive(true);
    sync_mode(ui, &state.borrow());
    update_ready(ui, &state.borrow());

    match result {
        Ok(()) => {
            set_status(ui, "Terminé", "success");
            ui.progress_bar.set_fraction(1.0);
            ui.progress_percent.set_text("100.0 %");
            ui.progress_stage.set_text("Clé USB créée avec succès");
            ui.progress_eta.set_text("");
            append_log(ui, "Flashage terminé avec succès.", LogLevel::Success);
            ui.toast
                .add_toast(adw::Toast::new("Clé USB créée avec succès."));
        }
        Err(err) if err.contains("annulée") => {
            set_status(ui, "Annulé", "idle");
            ui.progress_stage.set_text("Opération annulée");
            ui.progress_eta.set_text("");
            append_log(ui, "Opération annulée par l'utilisateur.", LogLevel::Info);
            ui.toast.add_toast(adw::Toast::new("Opération annulée."));
        }
        Err(err) => {
            set_status(ui, "Erreur", "error");
            ui.progress_stage.set_text("Échec du flashage");
            ui.progress_eta.set_text("");
            ui.progress_bar.add_css_class("progress-bar-error");
            append_log(ui, &format!("Échec du flashage : {err}"), LogLevel::Error);
            ui.toast.add_toast(adw::Toast::new("Échec du flashage."));
        }
    }

    refresh_drives(ui, state);
}

fn request_cancel(ui: &Rc<Ui>, state: &Rc<RefCell<State>>) {
    if state.borrow().is_flashing {
        state.borrow().cancel_flag.store(true, Ordering::SeqCst);
        ui.cancel_button.set_sensitive(false);
        append_log(
            ui,
            "Demande d'annulation envoyée au worker privilégié…",
            LogLevel::Error,
        );
    }
}

fn show_preview(ui: &Rc<Ui>, state: &Rc<RefCell<State>>) {
    let custom = build_windows_custom(ui, &state.borrow());
    let xml = debloat::generate_autounattend_xml(&custom);
    let ps1 = debloat::generate_debloat_powershell_script(&custom);

    let dialog = adw::Dialog::builder()
        .title("Aperçu des scripts Windows")
        .content_width(920)
        .content_height(660)
        .build();
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_hexpand(true);
    let switcher = gtk::StackSwitcher::new();
    switcher.set_stack(Some(&stack));
    header.set_title_widget(Some(&switcher));
    toolbar.add_top_bar(&header);

    let xml_view = code_view(&xml);
    let ps1_view = code_view(&ps1);
    let xml_scroll = gtk::ScrolledWindow::builder()
        .child(&xml_view)
        .hexpand(true)
        .vexpand(true)
        .build();
    let ps1_scroll = gtk::ScrolledWindow::builder()
        .child(&ps1_view)
        .hexpand(true)
        .vexpand(true)
        .build();
    stack.add_titled(&xml_scroll, Some("xml"), "autounattend.xml");
    stack.add_titled(&ps1_scroll, Some("ps1"), "noos_debloat.ps1");
    toolbar.set_content(Some(&stack));

    let copy_button = gtk::Button::with_label("Copier le code");
    let close_button = gtk::Button::with_label("Fermer");
    close_button.add_css_class("suggested-action");
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bottom.set_halign(gtk::Align::End);
    bottom.set_margin_top(8);
    bottom.set_margin_bottom(8);
    bottom.set_margin_start(12);
    bottom.set_margin_end(12);
    bottom.append(&copy_button);
    bottom.append(&close_button);
    toolbar.add_bottom_bar(&bottom);

    {
        let stack = stack.clone();
        let xml = xml.clone();
        let ps1 = ps1.clone();
        let toast = ui.toast.clone();
        let window = ui.window.clone();
        copy_button.connect_clicked(move |_| {
            let text = if stack.visible_child_name().as_deref() == Some("ps1") {
                &ps1
            } else {
                &xml
            };
            window.clipboard().set_text(text);
            toast.add_toast(adw::Toast::new("Script copié dans le presse-papiers."));
        });
    }
    {
        let dialog = dialog.clone();
        close_button.connect_clicked(move |_| {
            dialog.close();
        });
    }

    dialog.set_child(Some(&toolbar));
    dialog.present(Some(&ui.window));
}

fn code_view(content: &str) -> gtk::TextView {
    let buffer = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
    buffer.set_text(content);
    let view = gtk::TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::None)
        .build();
    view.add_css_class("code-view");
    view.set_top_margin(8);
    view.set_bottom_margin(8);
    view.set_left_margin(8);
    view.set_right_margin(8);
    view
}

fn set_status(ui: &Ui, text: &str, kind: &str) {
    ui.progress_status.set_text(text);
    ui.progress_status.remove_css_class("status-idle");
    ui.progress_status.remove_css_class("status-running");
    ui.progress_status.remove_css_class("status-success");
    ui.progress_status.remove_css_class("status-error");
    ui.progress_status.add_css_class(&format!("status-{kind}"));
}

fn append_log(ui: &Ui, message: &str, level: LogLevel) {
    let timestamp = glib::DateTime::now_local()
        .ok()
        .and_then(|dt| dt.format("%H:%M:%S").ok())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let tag = match level {
        LogLevel::Info => "info",
        LogLevel::Success => "success",
        LogLevel::Error => "error",
    };
    let mut end = ui.log_buffer.end_iter();
    ui.log_buffer
        .insert_with_tags_by_name(&mut end, &format!("[{timestamp}] {message}\n"), &[tag]);
    let mut end = ui.log_buffer.end_iter();
    ui.log_view.scroll_to_iter(&mut end, 0.0, false, 0.0, 0.0);
}

fn spawn_background<T, F, G>(task: F, on_done: G)
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
    G: FnOnce(T) + 'static,
{
    let (tx, rx) = async_channel::bounded::<T>(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(task());
    });
    glib::spawn_future_local(async move {
        if let Ok(value) = rx.recv().await {
            on_done(value);
        }
    });
}

fn escape_markup(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn mode_index(mode: FlashMode) -> u32 {
    match mode {
        FlashMode::LinuxDd => 0,
        FlashMode::WindowsUefi => 1,
    }
}

fn mode_from_index(index: u32) -> FlashMode {
    match index {
        1 => FlashMode::WindowsUefi,
        _ => FlashMode::LinuxDd,
    }
}

fn scheme_index(scheme: PartitionScheme) -> u32 {
    match scheme {
        PartitionScheme::Gpt => 0,
        PartitionScheme::Mbr => 1,
    }
}

fn scheme_from_index(index: u32) -> PartitionScheme {
    match index {
        1 => PartitionScheme::Mbr,
        _ => PartitionScheme::Gpt,
    }
}

fn fs_index(fs: FileSystemType) -> u32 {
    match fs {
        FileSystemType::Fat32 => 0,
        FileSystemType::Ntfs => 1,
    }
}

fn fs_from_index(index: u32) -> FileSystemType {
    match index {
        1 => FileSystemType::Ntfs,
        _ => FileSystemType::Fat32,
    }
}

fn format_progress_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.2} Gio", value / GIB)
    } else {
        format!("{:.1} Mio", value / MIB)
    }
}

fn format_eta(seconds: Option<u64>) -> String {
    match seconds {
        Some(0) | None => String::new(),
        Some(secs) if secs >= 60 => {
            format!(
                "Temps restant estimé : {} min {:02} s",
                secs / 60,
                secs % 60
            )
        }
        Some(secs) => format!("Temps restant estimé : {secs} s"),
    }
}
