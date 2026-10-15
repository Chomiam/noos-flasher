"use strict";

/**
 * Échappement HTML unique et sécurisé contre les injections XSS.
 * @param {unknown} str
 * @returns {string}
 */
function escapeHtml(str) {
  if (str === null || str === undefined) return "";
  return String(str)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

/**
 * Formate un nombre d'octets en chaîne lisible (Mio / Gio).
 * @param {number} bytes
 * @returns {string}
 */
function formatBytes(bytes) {
  if (!bytes || bytes <= 0) return "0.0 Mio";
  const gib = bytes / (1024 * 1024 * 1024);
  if (gib >= 1.0) {
    return `${gib.toFixed(2)} Gio`;
  }
  const mib = bytes / (1024 * 1024);
  return `${mib.toFixed(1)} Mio`;
}

/**
 * Formate une durée en secondes au format mm:ss.
 * @param {number | null | undefined} secs
 * @returns {string}
 */
function formatEta(secs) {
  if (secs === null || secs === undefined || secs <= 0) return "";
  const mins = Math.floor(secs / 60);
  const rem = secs % 60;
  if (mins > 0) {
    return `• Temps restant estimé : ${mins}m ${String(rem).padStart(2, "0")}s`;
  }
  return `• Temps restant estimé : ${rem}s`;
}

// Options statiques des menus déroulants sombres personnalisés
const MODE_OPTIONS = [
  {
    value: "linux_dd",
    label: "Mode DD — Écriture brute par blocs (Linux Hybride / IMG)",
  },
  {
    value: "windows_uefi",
    label: "Mode Windows UEFI — Partitionnement + Extraction + Debloat",
  },
];

const SCHEME_OPTIONS = [
  { value: "gpt", label: "GPT (UEFI moderne)" },
  { value: "mbr", label: "MBR (BIOS / UEFI-CSM)" },
];

const FS_OPTIONS = [
  { value: "fat32", label: "FAT32 (Standard UEFI + wimlib)" },
  { value: "ntfs", label: "NTFS (Fichiers > 4 Go natifs)" },
];

// État applicatif global du frontend
const appState = {
  drives: [],
  selectedDrive: null,
  isoInspection: null,
  capabilities: null,
  isFlashing: false,
  flashMode: "linux_dd",
  partitionScheme: "gpt",
  fsType: "fat32",
  debloatCatalog: [],
  enabledFeatures: new Set(),
  previewCache: {
    autounattend_xml: "",
    debloat_ps1: "",
    activeTab: "xml",
  },
};

// Accès sécurisé aux APIs Tauri v2 globales
function getTauriInvoke() {
  if (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke) {
    return window.__TAURI__.core.invoke;
  }
  return null;
}

function getTauriEventListen() {
  if (window.__TAURI__ && window.__TAURI__.event && window.__TAURI__.event.listen) {
    return window.__TAURI__.event.listen;
  }
  return null;
}

function getTauriDialogOpen() {
  if (window.__TAURI__ && window.__TAURI__.dialog && window.__TAURI__.dialog.open) {
    return window.__TAURI__.dialog.open;
  }
  return null;
}

/**
 * Ajoute une entrée horodatée au journal déroulant.
 * @param {string} message
 * @param {"info" | "error" | "success"} [level="info"]
 */
function appendLog(message, level = "info") {
  const container = document.getElementById("logs-output");
  if (!container) return;

  const now = new Date();
  const timestamp = now.toTimeString().split(" ")[0];

  const entry = document.createElement("div");
  entry.className = "log-entry";
  if (level === "error") entry.classList.add("log-error");
  if (level === "success") entry.classList.add("log-success");

  const timeSpan = document.createElement("span");
  timeSpan.className = "log-time";
  timeSpan.textContent = `[${timestamp}]`;

  const msgSpan = document.createElement("span");
  msgSpan.textContent = message;

  entry.appendChild(timeSpan);
  entry.appendChild(msgSpan);
  container.appendChild(entry);
  container.scrollTop = container.scrollHeight;
}

// ============================================================================
// SYSTÈME DE MENUS DÉROULANTS SOMBRES PERSONNALISÉS (ZÉRO POPUP BLANC GTK)
// ============================================================================

function closeAllDropdownMenus() {
  document.querySelectorAll(".dropdown-menu").forEach((menu) => {
    menu.classList.add("hidden");
  });
  document.querySelectorAll(".dropdown-trigger").forEach((btn) => {
    btn.classList.remove("open");
  });
}

/**
 * Configure un menu déroulant sombre personnalisé.
 */
function renderCustomDropdown({
  btnId,
  labelId,
  menuId,
  options,
  selectedValue,
  onSelect,
}) {
  const btn = document.getElementById(btnId);
  const labelEl = document.getElementById(labelId);
  const menuEl = document.getElementById(menuId);
  if (!btn || !labelEl || !menuEl) return;

  const currentOpt = options.find((o) => o.value === selectedValue) || options[0];
  labelEl.textContent = currentOpt ? currentOpt.label : "Aucun élément";

  const fragment = document.createDocumentFragment();
  for (const opt of options) {
    const item = document.createElement("div");
    item.className = `dropdown-option${opt.value === selectedValue ? " selected" : ""}`;
    item.dataset.value = opt.value;

    const textSpan = document.createElement("span");
    textSpan.textContent = opt.label;
    item.appendChild(textSpan);

    if (opt.sublabel) {
      const subSpan = document.createElement("span");
      subSpan.className = "dropdown-option-sub";
      subSpan.textContent = opt.sublabel;
      item.appendChild(subSpan);
    }

    item.addEventListener("click", (e) => {
      e.stopPropagation();
      closeAllDropdownMenus();
      onSelect(opt.value);
    });

    fragment.appendChild(item);
  }

  menuEl.replaceChildren(fragment);

  btn.onclick = (e) => {
    e.stopPropagation();
    if (btn.disabled) return;
    const isOpen = !menuEl.classList.contains("hidden");
    closeAllDropdownMenus();
    if (!isOpen && options.length > 0) {
      menuEl.classList.remove("hidden");
      btn.classList.add("open");
    }
  };
}

function renderStaticDropdowns() {
  renderCustomDropdown({
    btnId: "mode-dropdown-btn",
    labelId: "mode-dropdown-label",
    menuId: "mode-dropdown-menu",
    options: MODE_OPTIONS,
    selectedValue: appState.flashMode,
    onSelect: (val) => {
      appState.flashMode = val;
      renderStaticDropdowns();
      syncModeControlsVisibility();
    },
  });

  renderCustomDropdown({
    btnId: "scheme-dropdown-btn",
    labelId: "scheme-dropdown-label",
    menuId: "scheme-dropdown-menu",
    options: SCHEME_OPTIONS,
    selectedValue: appState.partitionScheme,
    onSelect: (val) => {
      appState.partitionScheme = val;
      renderStaticDropdowns();
    },
  });

  renderCustomDropdown({
    btnId: "fs-dropdown-btn",
    labelId: "fs-dropdown-label",
    menuId: "fs-dropdown-menu",
    options: FS_OPTIONS,
    selectedValue: appState.fsType,
    onSelect: (val) => {
      appState.fsType = val;
      renderStaticDropdowns();
      syncModeControlsVisibility();
    },
  });
}

// ============================================================================
// CONTRÔLES DE FENÊTRE PERSONNALISÉS (MINIMISER / MAXIMISER / QUITTER)
// ============================================================================

function setupWindowControls() {
  const invoke = getTauriInvoke();

  document.getElementById("btn-win-minimize")?.addEventListener("click", async () => {
    if (invoke) {
      await invoke("window_minimize").catch(() => {});
    }
  });

  document.getElementById("btn-win-maximize")?.addEventListener("click", async () => {
    if (invoke) {
      await invoke("window_toggle_maximize").catch(() => {});
    }
  });

  document.getElementById("btn-win-close")?.addEventListener("click", async () => {
    if (invoke) {
      await invoke("window_close").catch(() => {});
    }
  });

  // Double-clic sur la barre de titre pour maximiser/restaurer
  document.getElementById("titlebar-drag-area")?.addEventListener("dblclick", async (e) => {
    if (e.target.closest(".window-controls")) return;
    if (invoke) {
      await invoke("window_toggle_maximize").catch(() => {});
    }
  });
}

// ============================================================================
// CAPACITÉS SYSTÈME & DÉTECTION DES CLÉS USB
// ============================================================================

async function loadSystemCapabilities() {
  const invoke = getTauriInvoke();
  const bar = document.getElementById("sys-capabilities-bar");
  if (!invoke || !bar) return;

  try {
    const caps = await invoke("get_system_capabilities");
    appState.capabilities = caps;

    const items = [
      { name: "pkexec", ok: caps.pkexec },
      { name: "parted", ok: caps.parted },
      { name: "FAT32", ok: caps.mkfs_vfat },
      { name: "NTFS", ok: caps.mkfs_ntfs },
      { name: "7z", ok: caps.seven_zip },
      { name: "wimlib (embarqué)", ok: caps.wimlib_imagex },
    ];

    const fragment = document.createDocumentFragment();
    for (const item of items) {
      const span = document.createElement("span");
      span.className = `badge ${item.ok ? "badge-ok" : "badge-warn"}`;
      span.textContent = `${item.name} ${item.ok ? "✓" : " absent"}`;
      fragment.appendChild(span);
    }
    bar.replaceChildren(fragment);
  } catch (err) {
    appendLog(`Impossible de vérifier les outils système : ${err}`, "error");
  }
}

async function refreshDrives() {
  const invoke = getTauriInvoke();
  if (!invoke) return;

  const previousPath = appState.selectedDrive ? appState.selectedDrive.path : "";

  try {
    const drives = await invoke("get_removable_drives");
    appState.drives = Array.isArray(drives) ? drives : [];

    if (appState.drives.length === 0) {
      appState.selectedDrive = null;
      renderCustomDropdown({
        btnId: "drive-dropdown-btn",
        labelId: "drive-dropdown-label",
        menuId: "drive-dropdown-menu",
        options: [{ value: "", label: "Aucun périphérique USB amovible détecté" }],
        selectedValue: "",
        onSelect: () => {},
      });
    } else {
      const keep = appState.drives.find((d) => d.path === previousPath) || appState.drives[0];
      appState.selectedDrive = keep;

      const driveOptions = appState.drives.map((drv) => ({
        value: drv.path,
        label: `${drv.path} — ${drv.vendor} ${drv.model}`,
        sublabel: drv.size_formatted,
      }));

      renderCustomDropdown({
        btnId: "drive-dropdown-btn",
        labelId: "drive-dropdown-label",
        menuId: "drive-dropdown-menu",
        options: driveOptions,
        selectedValue: keep.path,
        onSelect: (chosenPath) => {
          const found = appState.drives.find((d) => d.path === chosenPath) || null;
          appState.selectedDrive = found;
          refreshDriveDropdownVisual();
          renderDriveDetails(found);
          updateActionButtonsState();
        },
      });
    }

    renderDriveDetails(appState.selectedDrive);
    updateActionButtonsState();
    appendLog(`${appState.drives.length} périphérique(s) USB amovible(s) détecté(s).`);
  } catch (err) {
    appendLog(`Erreur lors de l'énumération des disques : ${err}`, "error");
  }
}

function refreshDriveDropdownVisual() {
  const driveOptions = appState.drives.map((drv) => ({
    value: drv.path,
    label: `${drv.path} — ${drv.vendor} ${drv.model}`,
    sublabel: drv.size_formatted,
  }));
  const currentVal = appState.selectedDrive ? appState.selectedDrive.path : "";
  renderCustomDropdown({
    btnId: "drive-dropdown-btn",
    labelId: "drive-dropdown-label",
    menuId: "drive-dropdown-menu",
    options: driveOptions,
    selectedValue: currentVal,
    onSelect: (chosenPath) => {
      const found = appState.drives.find((d) => d.path === chosenPath) || null;
      appState.selectedDrive = found;
      refreshDriveDropdownVisual();
      renderDriveDetails(found);
      updateActionButtonsState();
    },
  });
}

function renderDriveDetails(drive) {
  const box = document.getElementById("drive-details-box");
  if (!box) return;

  if (!drive) {
    box.className = "info-box empty-state";
    box.textContent =
      "Aucun périphérique USB sélectionné. Les disques système (/, NVMe internes) sont masqués par sécurité.";
    return;
  }

  box.className = "info-box";
  const partCount = Array.isArray(drive.partitions) ? drive.partitions.length : 0;

  box.innerHTML = `
    <div class="meta-grid">
      <div class="meta-item">
        <span class="meta-label">Périphérique</span>
        <span class="meta-value"><code>${escapeHtml(drive.path)}</code> (${escapeHtml(drive.transport)})</span>
      </div>
      <div class="meta-item">
        <span class="meta-label">Modèle / Vendeur</span>
        <span class="meta-value">${escapeHtml(drive.vendor)} ${escapeHtml(drive.model)}</span>
      </div>
      <div class="meta-item">
        <span class="meta-label">Capacité Totale</span>
        <span class="meta-value">${escapeHtml(drive.size_formatted)}</span>
      </div>
      <div class="meta-item">
        <span class="meta-label">Points de montage (${partCount} part.)</span>
        <span class="meta-value">${escapeHtml(drive.mountpoint_display)}</span>
      </div>
    </div>
  `;
}

// ============================================================================
// SÉLECTION & INSPECTION D'IMAGE ISO / IMG
// ============================================================================

async function pickIsoFile() {
  const openDialog = getTauriDialogOpen();
  if (!openDialog) {
    appendLog("Plugin Tauri Dialog indisponible.", "error");
    return;
  }

  try {
    const selected = await openDialog({
      multiple: false,
      directory: false,
      title: "Sélectionner une image ISO ou IMG bootable",
      filters: [
        {
          name: "Images Disque Bootables (.iso, .img)",
          extensions: ["iso", "img", "ISO", "IMG"],
        },
      ],
    });

    if (!selected) return;
    const filePath = typeof selected === "string" ? selected : selected.path;
    if (filePath) {
      await inspectSelectedIso(filePath);
    }
  } catch (err) {
    appendLog(`Erreur lors de l'ouverture du sélecteur de fichier : ${err}`, "error");
  }
}

async function inspectSelectedIso(filePath) {
  const invoke = getTauriInvoke();
  const pathInput = document.getElementById("iso-path-input");
  if (!invoke || !pathInput) return;

  pathInput.value = filePath;
  appendLog(`Inspection de l'image : ${filePath}...`);

  try {
    const info = await invoke("inspect_iso", { path: filePath });
    appState.isoInspection = info;

    if (info.recommended_mode) appState.flashMode = info.recommended_mode;
    if (info.recommended_scheme) appState.partitionScheme = info.recommended_scheme;
    if (info.recommended_fs) appState.fsType = info.recommended_fs;

    const labelInput = document.getElementById("volume-label-input");
    if (labelInput && info.volume_label) {
      labelInput.value = info.volume_label;
    }

    renderStaticDropdowns();
    renderIsoDetails(info);
    syncModeControlsVisibility();
    updateActionButtonsState();
    appendLog(`Image détectée : ${info.kind_label} (${info.size_formatted}).`, "success");
  } catch (err) {
    appState.isoInspection = null;
    renderIsoDetails(null, String(err));
    updateActionButtonsState();
    appendLog(`Image invalide : ${err}`, "error");
  }
}

function renderIsoDetails(info, errorMsg = "") {
  const box = document.getElementById("iso-details-box");
  if (!box) return;

  if (!info) {
    box.className = "info-box empty-state";
    box.textContent = errorMsg || "Sélectionnez un fichier .iso ou .img valide.";
    return;
  }

  box.className = "info-box";
  const pillClass = info.is_windows_iso ? "iso-kind-windows" : "iso-kind-linux";
  const wimInfo = info.install_wim_formatted
    ? `${escapeHtml(info.install_wim_formatted)}${info.requires_wim_split_for_fat32 ? " (> 4 Gio)" : ""}`
    : "Non applicable";

  box.innerHTML = `
    <div>
      <span class="iso-kind-pill ${pillClass}">${escapeHtml(info.kind_label)}</span>
    </div>
    <div class="meta-grid">
      <div class="meta-item">
        <span class="meta-label">Fichier</span>
        <span class="meta-value">${escapeHtml(info.file_name)}</span>
      </div>
      <div class="meta-item">
        <span class="meta-label">Taille &amp; Label</span>
        <span class="meta-value">${escapeHtml(info.size_formatted)} — <code>${escapeHtml(info.volume_label)}</code></span>
      </div>
      <div class="meta-item">
        <span class="meta-label">Amorçage détecté</span>
        <span class="meta-value">${info.is_hybrid_mbr ? "MBR Hybride" : "Standard"} ${info.has_el_torito ? "+ El Torito" : ""} ${info.is_udf ? "+ UDF" : ""}</span>
      </div>
      <div class="meta-item">
        <span class="meta-label">Fichier install.wim</span>
        <span class="meta-value">${wimInfo}</span>
      </div>
    </div>
  `;
}

// ============================================================================
// CATALOGUE WINDOWS DEBLOAT & PERSONNALISATION COMPTE LOCAL
// ============================================================================

async function loadWindowsDebloatCatalog() {
  const invoke = getTauriInvoke();
  if (!invoke) return;

  try {
    const catalog = await invoke("get_windows_debloat_catalog");
    appState.debloatCatalog = Array.isArray(catalog) ? catalog : [];
    appState.enabledFeatures = new Set(
      appState.debloatCatalog.filter((f) => f.default_enabled).map((f) => f.id)
    );
    renderDebloatCatalog();
  } catch (err) {
    appendLog(`Erreur chargement du catalogue Debloat : ${err}`, "error");
  }
}

function renderDebloatCatalog() {
  const container = document.getElementById("debloat-catalog-container");
  const counterEl = document.getElementById("debloat-active-count");
  if (!container) return;

  const total = appState.debloatCatalog.length;
  const activeCount = appState.enabledFeatures.size;
  if (counterEl) {
    counterEl.textContent = `${activeCount} / ${total} actives`;
  }

  // Regrouper par catégorie
  const groups = new Map();
  for (const item of appState.debloatCatalog) {
    if (!groups.has(item.category)) {
      groups.set(item.category, []);
    }
    groups.get(item.category).push(item);
  }

  const fragment = document.createDocumentFragment();
  for (const [categoryName, items] of groups.entries()) {
    const groupDiv = document.createElement("div");
    groupDiv.className = "debloat-category-group";

    const header = document.createElement("div");
    header.className = "debloat-category-header";
    header.textContent = categoryName;
    groupDiv.appendChild(header);

    for (const feat of items) {
      const label = document.createElement("label");
      label.className = "debloat-item";

      const isChecked = appState.enabledFeatures.has(feat.id);

      label.innerHTML = `
        <div class="debloat-item-left">
          <input type="checkbox" data-feature-id="${escapeHtml(feat.id)}" ${isChecked ? "checked" : ""} />
          <div>
            <span class="check-title">${escapeHtml(feat.title)}</span>
            <span class="check-desc">${escapeHtml(feat.description)}</span>
          </div>
        </div>
        <span class="debloat-badge">${escapeHtml(feat.badge)}</span>
      `;

      groupDiv.appendChild(label);
    }

    fragment.appendChild(groupDiv);
  }

  container.replaceChildren(fragment);
}

function applyDebloatPreset(presetName) {
  if (presetName === "recommended") {
    appState.enabledFeatures = new Set(
      appState.debloatCatalog.filter((f) => f.default_enabled).map((f) => f.id)
    );
  } else if (presetName === "all") {
    appState.enabledFeatures = new Set(appState.debloatCatalog.map((f) => f.id));
  } else if (presetName === "minimal") {
    appState.enabledFeatures = new Set([
      "bypass_win11_hw",
      "bypass_ram_storage",
      "disable_bitlocker_auto",
    ]);
  } else if (presetName === "none") {
    appState.enabledFeatures = new Set();
  }
  renderDebloatCatalog();
}

/**
 * Construit l'objet `WindowsCustomConfig` à partir de l'état de l'interface.
 */
function buildWindowsCustomConfigFromUi() {
  const enabled = Boolean(document.getElementById("chk-enable-unattend")?.checked);
  const forceLocal = Boolean(document.getElementById("chk-force-local-account")?.checked);
  const createLocal = Boolean(document.getElementById("chk-create-local-user")?.checked);
  const localUsername =
    document.getElementById("local-username-input")?.value.trim() || "Utilisateur";
  const localPassword = document.getElementById("local-password-input")?.value || "";
  const autoLogon = Boolean(document.getElementById("chk-auto-logon")?.checked);
  const skipPrivacy = Boolean(document.getElementById("chk-skip-privacy")?.checked);
  const frenchLocale = Boolean(document.getElementById("chk-french-locale")?.checked);

  return {
    enabled,
    force_local_account: forceLocal,
    create_local_user: createLocal,
    local_username: localUsername,
    local_password: localPassword,
    auto_logon: autoLogon,
    skip_oobe_privacy: skipPrivacy,
    set_french_locale: frenchLocale,
    enabled_features: Array.from(appState.enabledFeatures),
  };
}

async function openScriptsPreviewModal() {
  const invoke = getTauriInvoke();
  const backdrop = document.getElementById("preview-modal-backdrop");
  const codeBox = document.getElementById("code-preview-content");
  if (!invoke || !backdrop || !codeBox) return;

  try {
    const winCustom = buildWindowsCustomConfigFromUi();
    const preview = await invoke("preview_windows_scripts", { windowsCustom: winCustom });
    appState.previewCache.autounattend_xml = preview.autounattend_xml;
    appState.previewCache.debloat_ps1 = preview.debloat_ps1;
    appState.previewCache.activeTab = "xml";

    renderPreviewTabContent();
    backdrop.classList.remove("hidden");
  } catch (err) {
    appendLog(`Erreur génération de l'aperçu : ${err}`, "error");
  }
}

function renderPreviewTabContent() {
  const codeBox = document.getElementById("code-preview-content");
  const btnXml = document.getElementById("btn-tab-unattend");
  const btnPs1 = document.getElementById("btn-tab-ps1");
  if (!codeBox || !btnXml || !btnPs1) return;

  const isXml = appState.previewCache.activeTab === "xml";
  codeBox.textContent = isXml
    ? appState.previewCache.autounattend_xml
    : appState.previewCache.debloat_ps1;

  btnXml.className = `btn ${isXml ? "btn-primary" : "btn-secondary"} btn-sm`;
  btnPs1.className = `btn ${!isXml ? "btn-primary" : "btn-secondary"} btn-sm`;
}

/**
 * Active ou grise les sélecteurs GPT/MBR, FAT32/NTFS et le Studio Windows selon le mode choisi.
 */
function syncModeControlsVisibility() {
  const schemeBtn = document.getElementById("scheme-dropdown-btn");
  const fsBtn = document.getElementById("fs-dropdown-btn");
  const labelInput = document.getElementById("volume-label-input");
  const warningBanner = document.getElementById("wim-warning-banner");
  const studioCard = document.getElementById("windows-studio-card");
  const customBody = document.getElementById("windows-custom-body");
  const unattendChk = document.getElementById("chk-enable-unattend");
  const createLocalChk = document.getElementById("chk-create-local-user");
  const userInputsRow = document.getElementById("local-user-inputs-row");

  const isWindowsMode = appState.flashMode === "windows_uefi";
  if (schemeBtn) schemeBtn.disabled = !isWindowsMode || appState.isFlashing;
  if (fsBtn) fsBtn.disabled = !isWindowsMode || appState.isFlashing;
  if (labelInput) labelInput.disabled = !isWindowsMode || appState.isFlashing;

  if (studioCard) {
    studioCard.classList.toggle("studio-disabled", !isWindowsMode);
  }
  if (customBody && unattendChk) {
    customBody.style.pointerEvents = isWindowsMode && unattendChk.checked ? "auto" : "none";
    customBody.style.opacity = isWindowsMode && unattendChk.checked ? "1" : "0.45";
  }
  if (userInputsRow && createLocalChk) {
    userInputsRow.style.opacity = createLocalChk.checked ? "1" : "0.4";
    userInputsRow.style.pointerEvents = createLocalChk.checked ? "auto" : "none";
  }

  const info = appState.isoInspection;
  if (
    warningBanner &&
    isWindowsMode &&
    info &&
    info.requires_wim_split_for_fat32 &&
    appState.fsType === "fat32"
  ) {
    warningBanner.classList.remove("hidden");
    if (info.wimlib_available) {
      warningBanner.textContent =
        "Note FAT32 : install.wim dépasse 4 Gio. Il sera automatiquement découpé en fichiers .swm (3800 Mio max) via wimlib-imagex.";
    } else {
      warningBanner.textContent =
        "Attention : install.wim dépasse 4 Gio et 'wimlib-imagex' (paquet wimtools) n'est pas installé. Sélectionnez NTFS ou installez wimtools.";
    }
  } else if (warningBanner) {
    warningBanner.classList.add("hidden");
  }
}

function updateActionButtonsState() {
  const btnOpenConfirm = document.getElementById("btn-open-confirm");
  const btnCancel = document.getElementById("btn-cancel-flash");
  const btnRefresh = document.getElementById("btn-refresh-drives");
  const btnPickIso = document.getElementById("btn-pick-iso");
  const driveBtn = document.getElementById("drive-dropdown-btn");
  const modeBtn = document.getElementById("mode-dropdown-btn");

  if (!btnOpenConfirm || !btnCancel) return;

  const canFlash =
    !appState.isFlashing &&
    appState.selectedDrive !== null &&
    appState.isoInspection !== null;

  btnOpenConfirm.disabled = !canFlash;
  btnCancel.classList.toggle("hidden", !appState.isFlashing);

  if (btnRefresh) btnRefresh.disabled = appState.isFlashing;
  if (btnPickIso) btnPickIso.disabled = appState.isFlashing;
  if (driveBtn) driveBtn.disabled = appState.isFlashing;
  if (modeBtn) modeBtn.disabled = appState.isFlashing;
  syncModeControlsVisibility();
}

// ============================================================================
// MODALE DE CONFIRMATION & EXÉCUTION DU FLASHAGE
// ============================================================================

function openConfirmationModal() {
  if (!appState.selectedDrive || !appState.isoInspection) return;

  const backdrop = document.getElementById("confirm-modal-backdrop");
  const summary = document.getElementById("modal-summary-content");
  const labelInput = document.getElementById("volume-label-input");

  if (!backdrop || !summary || !labelInput) return;

  const drv = appState.selectedDrive;
  const iso = appState.isoInspection;
  const isWinMode = appState.flashMode === "windows_uefi";
  const winCustom = buildWindowsCustomConfigFromUi();

  const modeDesc = isWinMode
    ? `Windows UEFI (${appState.partitionScheme.toUpperCase()} / ${appState.fsType.toUpperCase()})`
    : "Mode DD (Écriture brute bloc à bloc + sync)";

  const customDesc =
    isWinMode && winCustom.enabled
      ? `Actif — Compte local '${winCustom.local_username}' + ${winCustom.enabled_features.length} patchs Debloat`
      : "Désactivé (Image d'origine non modifiée)";

  summary.innerHTML = `
    <div class="modal-row">
      <span class="meta-label">Cible matériel</span>
      <span class="meta-value"><code>${escapeHtml(drv.path)}</code> — ${escapeHtml(drv.vendor)} ${escapeHtml(drv.model)}</span>
    </div>
    <div class="modal-row">
      <span class="meta-label">Capacité de la clé</span>
      <span class="meta-value">${escapeHtml(drv.size_formatted)}</span>
    </div>
    <div class="modal-row">
      <span class="meta-label">Image source</span>
      <span class="meta-value">${escapeHtml(iso.file_name)} (${escapeHtml(iso.size_formatted)})</span>
    </div>
    <div class="modal-row">
      <span class="meta-label">Mode d'écriture</span>
      <span class="meta-value">${escapeHtml(modeDesc)}</span>
    </div>
    ${
      isWinMode
        ? `<div class="modal-row">
            <span class="meta-label">Label du volume</span>
            <span class="meta-value"><code>${escapeHtml(labelInput.value || "NOOS_USB")}</code></span>
          </div>
          <div class="modal-row">
            <span class="meta-label">Debloat &amp; Compte Local</span>
            <span class="meta-value">${escapeHtml(customDesc)}</span>
          </div>`
        : ""
    }
  `;

  backdrop.classList.remove("hidden");
}

function closeConfirmationModal() {
  const backdrop = document.getElementById("confirm-modal-backdrop");
  if (backdrop) backdrop.classList.add("hidden");
}

async function confirmAndStartFlash() {
  closeConfirmationModal();

  const invoke = getTauriInvoke();
  if (!invoke || !appState.selectedDrive || !appState.isoInspection) return;

  const volumeLabel = document.getElementById("volume-label-input")?.value || "NOOS_USB";
  const windowsCustom =
    appState.flashMode === "windows_uefi" ? buildWindowsCustomConfigFromUi() : null;

  appState.isFlashing = true;
  updateActionButtonsState();
  setProgressUiState({
    stage: "Initialisation du flashage...",
    percent: 0,
    speed_mbps: 0,
    bytes_processed: 0,
    total_bytes: appState.isoInspection.size_bytes,
    eta_seconds: null,
    statusType: "running",
    statusText: "En cours",
  });

  appendLog(
    `Démarrage du flashage sur ${appState.selectedDrive.path} (Mode: ${appState.flashMode})...`
  );

  try {
    await invoke("start_flash", {
      devicePath: appState.selectedDrive.path,
      isoPath: appState.isoInspection.path,
      mode: appState.flashMode,
      partitionScheme: appState.partitionScheme,
      fsType: appState.fsType,
      volumeLabel,
      windowsCustom,
    });
  } catch (err) {
    setProgressUiState({
      stage: `Échec : ${err}`,
      percent: 0,
      speed_mbps: 0,
      bytes_processed: 0,
      total_bytes: appState.isoInspection.size_bytes,
      eta_seconds: null,
      statusType: "error",
      statusText: "Erreur",
    });
    appendLog(`Échec du flashage : ${err}`, "error");
  } finally {
    appState.isFlashing = false;
    updateActionButtonsState();
    await refreshDrives();
  }
}

async function cancelOngoingFlash() {
  const invoke = getTauriInvoke();
  if (!invoke || !appState.isFlashing) return;
  appendLog("Demande d'annulation envoyée au worker privilégié...", "error");
  try {
    await invoke("cancel_flash");
  } catch (err) {
    appendLog(`Erreur lors de l'annulation : ${err}`, "error");
  }
}

function setProgressUiState({
  stage,
  percent,
  speed_mbps,
  bytes_processed,
  total_bytes,
  eta_seconds,
  statusType,
  statusText,
}) {
  const stageEl = document.getElementById("progress-stage-text");
  const etaEl = document.getElementById("progress-eta-text");
  const speedEl = document.getElementById("progress-speed-pill");
  const percentEl = document.getElementById("progress-percent-pill");
  const fillEl = document.getElementById("progress-bar-fill");
  const bytesEl = document.getElementById("progress-bytes-text");
  const statusEl = document.getElementById("progress-status-badge");

  const clampedPercent = Math.max(0, Math.min(100, Number(percent) || 0));

  if (stageEl && stage) stageEl.textContent = stage;
  if (etaEl) etaEl.textContent = formatEta(eta_seconds);
  if (speedEl) speedEl.textContent = `${(Number(speed_mbps) || 0).toFixed(1)} Mo/s`;
  if (percentEl) percentEl.textContent = `${clampedPercent.toFixed(1)}%`;

  if (fillEl) {
    fillEl.style.width = `${clampedPercent}%`;
    fillEl.classList.remove("fill-success", "fill-error");
    if (statusType === "success") fillEl.classList.add("fill-success");
    if (statusType === "error") fillEl.classList.add("fill-error");
  }

  if (bytesEl && total_bytes > 0) {
    bytesEl.textContent = `${formatBytes(bytes_processed)} / ${formatBytes(total_bytes)}`;
  }

  if (statusEl && statusText) {
    statusEl.className = `status-tag status-${statusType || "idle"}`;
    statusEl.textContent = statusText;
  }
}

async function setupProgressListener() {
  const listen = getTauriEventListen();
  if (!listen) return;

  await listen("flash-progress", (event) => {
    const payload = event.payload;
    if (!payload) return;

    if (payload.log) {
      const level = payload.error ? "error" : payload.completed ? "success" : "info";
      appendLog(payload.log, level);
    }

    const statusType = payload.error
      ? "error"
      : payload.completed
        ? "success"
        : "running";
    const statusText = payload.error
      ? "Erreur"
      : payload.completed
        ? "Terminé"
        : "En cours";

    setProgressUiState({
      stage: payload.stage,
      percent: payload.percent,
      speed_mbps: payload.speed_mbps,
      bytes_processed: payload.bytes_processed,
      total_bytes: payload.total_bytes,
      eta_seconds: payload.eta_seconds,
      statusType,
      statusText,
    });
  });
}

// Initialisation au chargement du DOM
window.addEventListener("DOMContentLoaded", async () => {
  setupWindowControls();
  renderStaticDropdowns();

  // Fermer les menus déroulants personnalisés lors d'un clic à l'extérieur
  document.addEventListener("click", () => {
    closeAllDropdownMenus();
  });

  document.getElementById("btn-refresh-drives")?.addEventListener("click", refreshDrives);
  document.getElementById("btn-pick-iso")?.addEventListener("click", pickIsoFile);

  document.getElementById("chk-enable-unattend")?.addEventListener("change", syncModeControlsVisibility);
  document.getElementById("chk-create-local-user")?.addEventListener("change", syncModeControlsVisibility);

  // Délégation d'événements pour les 22 cases à cocher du catalogue Debloat
  document.getElementById("debloat-catalog-container")?.addEventListener("change", (e) => {
    const chk = e.target.closest("input[data-feature-id]");
    if (!chk) return;
    const featId = chk.dataset.featureId;
    if (chk.checked) {
      appState.enabledFeatures.add(featId);
    } else {
      appState.enabledFeatures.delete(featId);
    }
    const counterEl = document.getElementById("debloat-active-count");
    if (counterEl) {
      counterEl.textContent = `${appState.enabledFeatures.size} / ${appState.debloatCatalog.length} actives`;
    }
  });

  // Boutons de presets Debloat
  document.getElementById("btn-preset-recommended")?.addEventListener("click", () => applyDebloatPreset("recommended"));
  document.getElementById("btn-preset-all")?.addEventListener("click", () => applyDebloatPreset("all"));
  document.getElementById("btn-preset-minimal")?.addEventListener("click", () => applyDebloatPreset("minimal"));
  document.getElementById("btn-preset-none")?.addEventListener("click", () => applyDebloatPreset("none"));

  // Modale d'aperçu des scripts autounattend.xml / noos_debloat.ps1
  document.getElementById("btn-preview-scripts")?.addEventListener("click", openScriptsPreviewModal);
  document.getElementById("btn-close-preview")?.addEventListener("click", () => {
    document.getElementById("preview-modal-backdrop")?.classList.add("hidden");
  });
  document.getElementById("btn-tab-unattend")?.addEventListener("click", () => {
    appState.previewCache.activeTab = "xml";
    renderPreviewTabContent();
  });
  document.getElementById("btn-tab-ps1")?.addEventListener("click", () => {
    appState.previewCache.activeTab = "ps1";
    renderPreviewTabContent();
  });
  document.getElementById("btn-copy-preview")?.addEventListener("click", async (e) => {
    const codeBox = document.getElementById("code-preview-content");
    if (!codeBox) return;
    try {
      await navigator.clipboard.writeText(codeBox.textContent || "");
      const btn = e.currentTarget;
      const prev = btn.textContent;
      btn.textContent = "Copié ✓";
      setTimeout(() => {
        btn.textContent = prev;
      }, 1500);
    } catch (_) {
      // Ignorer
    }
  });

  // Modale de confirmation de flash
  document.getElementById("btn-open-confirm")?.addEventListener("click", openConfirmationModal);
  document.getElementById("btn-modal-cancel")?.addEventListener("click", closeConfirmationModal);
  document.getElementById("btn-modal-confirm")?.addEventListener("click", confirmAndStartFlash);
  document.getElementById("btn-cancel-flash")?.addEventListener("click", cancelOngoingFlash);

  document.getElementById("btn-clear-logs")?.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    const container = document.getElementById("logs-output");
    if (container) container.replaceChildren();
  });

  document.getElementById("btn-copy-logs")?.addEventListener("click", async (e) => {
    e.preventDefault();
    e.stopPropagation();
    const container = document.getElementById("logs-output");
    if (!container) return;
    try {
      await navigator.clipboard.writeText(container.innerText);
      const btn = e.currentTarget;
      const prev = btn.textContent;
      btn.textContent = "Copié ✓";
      setTimeout(() => {
        btn.textContent = prev;
      }, 1500);
    } catch (_) {
      // Ignorer
    }
  });

  appendLog("Noos Flasher initialisé. Studio Debloat Windows & isolation disques actifs.");
  await setupProgressListener();
  await loadSystemCapabilities();
  await loadWindowsDebloatCatalog();
  await refreshDrives();
  syncModeControlsVisibility();
});
