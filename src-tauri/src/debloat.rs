use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Descripteur d'une fonction de debloat / patch Windows exposée au frontend pour configuration granulaire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebloatFeatureDescriptor {
    pub id: String,
    pub category: String,
    pub title: String,
    pub description: String,
    pub badge: String,
    pub default_enabled: bool,
}

/// Configuration complète de personnalisation Windows (Unattend + Compte Local + Debloat).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsCustomConfig {
    /// Active la génération de `autounattend.xml` et des scripts `$OEM$` sur la clé USB.
    pub enabled: bool,
    /// Forcer l'utilisation d'un compte local (`OOBE\BypassNRO = 1` + masquage Microsoft Account).
    pub force_local_account: bool,
    /// Créer automatiquement un compte utilisateur Administrateur local pendant le formatage/installation.
    pub create_local_user: bool,
    /// Nom d'utilisateur du compte local Windows (ex: "Admin", "Utilisateur").
    pub local_username: String,
    /// Mot de passe optionnel du compte local (vide = sans mot de passe).
    pub local_password: String,
    /// Connexion automatique au bureau au premier démarrage.
    pub auto_logon: bool,
    /// Ignorer toutes les questions de confidentialité et d'EULA lors de l'OOBE.
    pub skip_oobe_privacy: bool,
    /// Pré-configurer la langue et le clavier en Français (fr-FR) durant l'OOBE.
    pub set_french_locale: bool,
    /// Liste des identifiants de fonctions de debloat/patchs activés.
    pub enabled_features: Vec<String>,
}

impl Default for WindowsCustomConfig {
    fn default() -> Self {
        let default_features = get_debloat_catalog()
            .into_iter()
            .filter(|f| f.default_enabled)
            .map(|f| f.id)
            .collect();

        Self {
            enabled: true,
            force_local_account: true,
            create_local_user: true,
            local_username: "Utilisateur".to_string(),
            local_password: String::new(),
            auto_logon: true,
            skip_oobe_privacy: true,
            set_french_locale: true,
            enabled_features: default_features,
        }
    }
}

impl WindowsCustomConfig {
    pub fn has_feature(&self, id: &str) -> bool {
        self.enabled_features.iter().any(|f| f == id)
    }
}

/// Catalogue exhaustif des 22 fonctions de debloat et patchs système (inspiré de Rufus, Win11Debloat et Chris Titus WinUtil).
pub fn get_debloat_catalog() -> Vec<DebloatFeatureDescriptor> {
    vec![
        // --- Catégorie 1 : Contournements Matériels & Sécurité (Style Rufus) ---
        DebloatFeatureDescriptor {
            id: "bypass_win11_hw".to_string(),
            category: "Matériel & Installation (Rufus)".to_string(),
            title: "Contourner TPM 2.0, Secure Boot & CPU".to_string(),
            description: "Ajoute les clés LabConfig (BypassTPMCheck, BypassSecureBootCheck, BypassCPUCheck) pour installer Windows 11 sur tout PC.".to_string(),
            badge: "WinPE / LabConfig".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "bypass_ram_storage".to_string(),
            category: "Matériel & Installation (Rufus)".to_string(),
            title: "Contourner les limites de RAM (4 Go) et Stockage (64 Go)".to_string(),
            description: "Désactive la vérification minimale de mémoire vive et d'espace disque lors du démarrage de Windows Setup.".to_string(),
            badge: "WinPE / LabConfig".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "disable_bitlocker_auto".to_string(),
            category: "Matériel & Installation (Rufus)".to_string(),
            title: "Désactiver le chiffrement automatique BitLocker".to_string(),
            description: "Active PreventDeviceEncryption=1 pour empêcher Windows 11 24H2 de chiffrer silencieusement les disques au premier boot.".to_string(),
            badge: "Registre HKLM".to_string(),
            default_enabled: true,
        },

        // --- Catégorie 2 : Suppression IA, Copilot, Recall & Recherche Web ---
        DebloatFeatureDescriptor {
            id: "remove_copilot".to_string(),
            category: "Intelligence Artificielle & Recherche".to_string(),
            title: "Supprimer et bloquer Microsoft Copilot".to_string(),
            description: "Désactive Windows Copilot via GPO (TurnOffWindowsCopilot=1), retire le bouton de la barre des tâches et purge le paquet AppX Copilot.".to_string(),
            badge: "GPO + AppX".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "disable_windows_recall".to_string(),
            category: "Intelligence Artificielle & Recherche".to_string(),
            title: "Désactiver Windows Recall & Captures IA (24H2)".to_string(),
            description: "Bloque l'analyse IA d'activité et l'enregistrement continu de captures d'écran (DisableAIDataAnalysis=1, TurnOffSavingSnapshots=1).".to_string(),
            badge: "GPO WindowsAI".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "disable_bing_search".to_string(),
            category: "Intelligence Artificielle & Recherche".to_string(),
            title: "Désactiver Bing & les résultats Web du menu Démarrer".to_string(),
            description: "Rend la recherche du menu Démarrer 100% locale et instantanée (DisableSearchBoxSuggestions=1, ConnectedSearchUseWeb=0).".to_string(),
            badge: "GPO Explorer".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "remove_cortana".to_string(),
            category: "Intelligence Artificielle & Recherche".to_string(),
            title: "Désactiver et désinstaller Cortana".to_string(),
            description: "Bloque AllowCortana=0 par stratégie de groupe et supprime le paquet UWP provisionné de l'assistant Cortana.".to_string(),
            badge: "GPO + AppX".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "disable_widgets_news".to_string(),
            category: "Intelligence Artificielle & Recherche".to_string(),
            title: "Supprimer les Widgets & Actualités MSN".to_string(),
            description: "Désactive le panneau Widgets/Météo/Bourse sur la barre des tâches (AllowNewsAndInterests=0, TaskbarDa=0).".to_string(),
            badge: "GPO Dsh".to_string(),
            default_enabled: true,
        },

        // --- Catégorie 3 : Debloat Applications & Bloatware (Style Win11Debloat) ---
        DebloatFeatureDescriptor {
            id: "remove_consumer_apps".to_string(),
            category: "Applications & Bloatwares (Win11Debloat)".to_string(),
            title: "Purger les applications préinstallées (Clipchamp, Solitaire, Météo, News, Cartes...)".to_string(),
            description: "Supprime de l'image Windows les paquets provisionnés : Clipchamp, Solitaire, BingNews, BingWeather, BingFinance, People, Maps, GetHelp, Tips, FeedbackHub, ToDo.".to_string(),
            badge: "AppX Provisioned".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "block_sponsored_apps".to_string(),
            category: "Applications & Bloatwares (Win11Debloat)".to_string(),
            title: "Bloquer le téléchargement silencieux d'apps sponsorisées (TikTok, Candy Crush, Spotify)".to_string(),
            description: "Active DisableWindowsConsumerFeatures=1 et SilentInstalledAppsEnabled=0 pour interdire l'installation automatique de bloatwares tiers.".to_string(),
            badge: "GPO CloudContent".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "remove_onedrive".to_string(),
            category: "Applications & Bloatwares (Win11Debloat)".to_string(),
            title: "Désinstaller et désactiver Microsoft OneDrive".to_string(),
            description: "Retire OneDriveSetup du profil utilisateur par défaut, désinstalle le client OneDrive et bloque la synchronisation forcée (DisableFileSyncNGSC=1).".to_string(),
            badge: "Setup + GPO".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "remove_office_outlook_hub".to_string(),
            category: "Applications & Bloatwares (Win11Debloat)".to_string(),
            title: "Supprimer Microsoft 365 Hub, le nouvel Outlook Web & Teams Personnel".to_string(),
            description: "Purge les lanceurs promotionnels MicrosoftOfficeHub, OutlookForWindows et l'application Teams grand public préinstallée.".to_string(),
            badge: "AppX Provisioned".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "remove_telemetry_edge_bloat".to_string(),
            category: "Applications & Bloatwares (Win11Debloat)".to_string(),
            title: "Alléger Microsoft Edge (Barre latérale, Shopping, 1er lancement & tâche de fond)".to_string(),
            description: "Désactive l'assistant d'achat Edge, la barre latérale MSN, l'écran promotionnel de premier lancement et l'exécution en arrière-plan.".to_string(),
            badge: "GPO Edge".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "remove_xbox_gamebar".to_string(),
            category: "Applications & Bloatwares (Win11Debloat)".to_string(),
            title: "Supprimer les applications Xbox & GameOverlay (Optionnel)".to_string(),
            description: "Désinstalle GamingApp et XboxGamingOverlay. Laissez décoché si vous utilisez le PC pour jouer (Xbox Game Pass / manettes).".to_string(),
            badge: "AppX Gaming".to_string(),
            default_enabled: false,
        },

        // --- Catégorie 4 : Confidentialité, Télémétrie & Services (Style Chris Titus WinUtil) ---
        DebloatFeatureDescriptor {
            id: "disable_telemetry_services".to_string(),
            category: "Confidentialité & Télémétrie (WinUtil)".to_string(),
            title: "Désactiver la télémétrie Windows & les services DiagTrack / dmwappush".to_string(),
            description: "Force AllowTelemetry=0, bloque les collecteurs de diagnostic et désactive les services système DiagTrack et dmwappushservice.".to_string(),
            badge: "Services + GPO".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "disable_advertising_tracking".to_string(),
            category: "Confidentialité & Télémétrie (WinUtil)".to_string(),
            title: "Bloquer l'identifiant publicitaire, le suivi d'activité & les pubs d'écran de verrouillage".to_string(),
            description: "Désactive AdvertisingInfo, l'historique d'activité cloud et les publicités Windows Spotlight sur l'écran de verrouillage.".to_string(),
            badge: "Registre Privacy".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "disable_wi_fi_sense_delivery".to_string(),
            category: "Confidentialité & Télémétrie (WinUtil)".to_string(),
            title: "Désactiver le partage P2P Windows Update (Delivery Optimization)".to_string(),
            description: "Empêche Windows d'utiliser votre bande passante Internet pour envoyer des mises à jour à d'autres PC (DODownloadMode=0).".to_string(),
            badge: "GPO Réseau".to_string(),
            default_enabled: true,
        },

        // --- Catégorie 5 : Interface, Explorateur & Performances ---
        DebloatFeatureDescriptor {
            id: "classic_context_menu".to_string(),
            category: "Interface & Optimisations Système".to_string(),
            title: "Restaurer le menu contextuel clic-droit classique (Windows 10)".to_string(),
            description: "Active le menu clic-droit complet sur Windows 11 sans avoir à cliquer sur 'Afficher plus d'options'.".to_string(),
            badge: "CLSID Shell".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "explorer_power_user".to_string(),
            category: "Interface & Optimisations Système".to_string(),
            title: "Explorateur Pro : Afficher les extensions de fichiers & ouvrir sur 'Ce PC'".to_string(),
            description: "Affiche les extensions (.exe, .txt, .iso) par défaut et ouvre l'Explorateur directement sur 'Ce PC' au lieu de 'Accueil'.".to_string(),
            badge: "Registre Explorer".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "clean_taskbar_start".to_string(),
            category: "Interface & Optimisations Système".to_string(),
            title: "Épurer la barre des tâches et le menu Démarrer".to_string(),
            description: "Masque les boutons Vue des tâches et Chat, et supprime les recommandations publicitaires du menu Démarrer.".to_string(),
            badge: "Registre Shell".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "disable_bing_lockscreen_tips".to_string(),
            category: "Interface & Optimisations Système".to_string(),
            title: "Bloquer les rappels plein écran 'Terminer la configuration de Windows'".to_string(),
            description: "Désactive les écrans de harcèlement post-installation incitant à lier un compte Microsoft ou souscrire à Microsoft 365.".to_string(),
            badge: "Registre OOBE".to_string(),
            default_enabled: true,
        },
        DebloatFeatureDescriptor {
            id: "high_perf_gaming_tweaks".to_string(),
            category: "Interface & Optimisations Système".to_string(),
            title: "Optimisations Performances (Profil Haute Performance & arrêt GameDVR)".to_string(),
            description: "Active le schéma d'alimentation Haute Performance et désactive la capture vidéo GameDVR en arrière-plan pour réduire la latence.".to_string(),
            badge: "PowerCfg + DVR".to_string(),
            default_enabled: false,
        },
    ]
}

/// Échappe les caractères spéciaux XML (`&`, `<`, `>`, `"`, `'`).
fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Nettoie le nom d'utilisateur local Windows (caractères interdits par NetBIOS/SAM éliminés, max 20 chars).
pub fn sanitize_windows_username(raw: &str) -> String {
    let forbidden = ['"', '/', '\\', '[', ']', ':', ';', '|', '=', ',', '+', '*', '?', '<', '>'];
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| !forbidden.contains(c) && !c.is_control())
        .take(20)
        .collect();

    if cleaned.is_empty() {
        "Utilisateur".to_string()
    } else {
        cleaned
    }
}

/// Génère le fichier `autounattend.xml` complet (phases `windowsPE`, `specialize`, `oobeSystem`).
pub fn generate_autounattend_xml(cfg: &WindowsCustomConfig) -> String {
    let safe_user = escape_xml(&sanitize_windows_username(&cfg.local_username));
    let safe_pass = escape_xml(&cfg.local_password);

    // Commandes RunSynchronous lors de la phase windowsPE (contournements matériels Rufus)
    let mut winpe_commands = Vec::new();
    let mut order = 1;

    if cfg.has_feature("bypass_win11_hw") {
        for (key, val) in [
            ("BypassTPMCheck", "1"),
            ("BypassSecureBootCheck", "1"),
            ("BypassCPUCheck", "1"),
        ] {
            winpe_commands.push(format!(
                r#"        <RunSynchronousCommand wcm:action="add">
          <Order>{order}</Order>
          <Path>reg.exe add "HKLM\SYSTEM\Setup\LabConfig" /v {key} /t REG_DWORD /d {val} /f</Path>
        </RunSynchronousCommand>"#
            ));
            order += 1;
        }
        winpe_commands.push(format!(
            r#"        <RunSynchronousCommand wcm:action="add">
          <Order>{order}</Order>
          <Path>reg.exe add "HKLM\SYSTEM\Setup\MoSetup" /v AllowUpgradesWithUnsupportedTPMOrCPU /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>"#
        ));
        order += 1;
    }

    if cfg.has_feature("bypass_ram_storage") {
        for key in ["BypassRAMCheck", "BypassStorageCheck"] {
            winpe_commands.push(format!(
                r#"        <RunSynchronousCommand wcm:action="add">
          <Order>{order}</Order>
          <Path>reg.exe add "HKLM\SYSTEM\Setup\LabConfig" /v {key} /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>"#
            ));
            order += 1;
        }
    }

    let winpe_run_sync = if winpe_commands.is_empty() {
        String::new()
    } else {
        format!(
            "      <RunSynchronous>\n{}\n      </RunSynchronous>",
            winpe_commands.join("\n")
        )
    };

    // Commandes RunSynchronous lors de la phase specialize (BypassNRO, BitLocker, exécution de noos_debloat.ps1)
    let mut spec_commands = Vec::new();
    let mut spec_order = 1;

    if cfg.force_local_account {
        spec_commands.push(format!(
            r#"        <RunSynchronousCommand wcm:action="add">
          <Order>{spec_order}</Order>
          <Path>reg.exe add "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\OOBE" /v BypassNRO /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>"#
        ));
        spec_order += 1;
    }

    if cfg.has_feature("disable_bitlocker_auto") {
        spec_commands.push(format!(
            r#"        <RunSynchronousCommand wcm:action="add">
          <Order>{spec_order}</Order>
          <Path>reg.exe add "HKLM\SYSTEM\CurrentControlSet\Control\BitLocker" /v PreventDeviceEncryption /t REG_DWORD /d 1 /f</Path>
        </RunSynchronousCommand>"#
        ));
        spec_order += 1;
    }

    // Appel du script de debloat PowerShell copié par Windows Setup depuis sources/$OEM$/$$/Setup/Scripts/noos_debloat.ps1
    spec_commands.push(format!(
        r#"        <RunSynchronousCommand wcm:action="add">
          <Order>{spec_order}</Order>
          <Path>powershell.exe -NoProfile -ExecutionPolicy Bypass -File "C:\Windows\Setup\Scripts\noos_debloat.ps1"</Path>
        </RunSynchronousCommand>"#
    ));

    let spec_run_sync = format!(
        "      <RunSynchronous>\n{}\n      </RunSynchronous>",
        spec_commands.join("\n")
    );

    // Section langue / clavier fr-FR optionnelle en phase oobeSystem
    let oobe_intl_block = if cfg.set_french_locale {
        r#"    <component name="Microsoft-Windows-International-Core" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <InputLocale>040c:0000040c</InputLocale>
      <SystemLocale>fr-FR</SystemLocale>
      <UILanguage>fr-FR</UILanguage>
      <UserLocale>fr-FR</UserLocale>
    </component>"#
    } else {
        ""
    };

    // Section OOBE (masquage compte en ligne, EULA, questions de confidentialité)
    let hide_online = if cfg.force_local_account {
        "true"
    } else {
        "false"
    };
    let protect_pc = if cfg.skip_oobe_privacy { "3" } else { "1" };
    let hide_eula = if cfg.skip_oobe_privacy {
        "true"
    } else {
        "false"
    };

    // Création du compte utilisateur local Administrateur + AutoLogon optionnel
    let user_accounts_block = if cfg.create_local_user {
        let auto_logon_xml = if cfg.auto_logon {
            format!(
                r#"      <AutoLogon>
        <Password>
          <Value>{safe_pass}</Value>
          <PlainText>true</PlainText>
        </Password>
        <Enabled>true</Enabled>
        <LogonCount>1</LogonCount>
        <Username>{safe_user}</Username>
      </AutoLogon>"#
            )
        } else {
            String::new()
        };

        format!(
            r#"{auto_logon_xml}
      <UserAccounts>
        <LocalAccounts>
          <LocalAccount wcm:action="add">
            <Password>
              <Value>{safe_pass}</Value>
              <PlainText>true</PlainText>
            </Password>
            <Description>Compte local créé par Noos Flasher</Description>
            <DisplayName>{safe_user}</DisplayName>
            <Group>Administrators</Group>
            <Name>{safe_user}</Name>
          </LocalAccount>
        </LocalAccounts>
      </UserAccounts>"#
        )
    } else {
        String::new()
    };

    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
  <settings pass="windowsPE">
    <component name="Microsoft-Windows-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
{winpe_run_sync}
      <UserData>
        <AcceptEula>true</AcceptEula>
      </UserData>
    </component>
  </settings>
  <settings pass="specialize">
    <component name="Microsoft-Windows-Deployment" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
{spec_run_sync}
    </component>
  </settings>
  <settings pass="oobeSystem">
{oobe_intl_block}
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <OOBE>
        <HideEULAPage>{hide_eula}</HideEULAPage>
        <HideOnlineAccountScreens>{hide_online}</HideOnlineAccountScreens>
        <HideWirelessSetupInOOBE>{hide_online}</HideWirelessSetupInOOBE>
        <ProtectYourPC>{protect_pc}</ProtectYourPC>
      </OOBE>
{user_accounts_block}
      <FirstLogonCommands>
        <SynchronousCommand wcm:action="add">
          <Order>1</Order>
          <Description>Application finale des optimisations Noos Debloat</Description>
          <CommandLine>powershell.exe -NoProfile -ExecutionPolicy Bypass -File "C:\Windows\Setup\Scripts\noos_debloat.ps1"</CommandLine>
        </SynchronousCommand>
      </FirstLogonCommands>
    </component>
  </settings>
</unattend>
"#
    )
}

/// Génère le script PowerShell `noos_debloat.ps1` correspondant exactement aux fonctions cochées par l'utilisateur.
pub fn generate_debloat_powershell_script(cfg: &WindowsCustomConfig) -> String {
    let mut ps = String::with_capacity(8192);

    ps.push_str(
        r#"# ==============================================================================
# Noos Flasher — Script d'optimisation et de Debloat Windows 10/11
# Généré automatiquement lors du flashage de la clé USB
# ==============================================================================
$ErrorActionPreference = 'SilentlyContinue'
Start-Transcript -Path "C:\Windows\Temp\noos_debloat.log" -Append

function Set-RegValue {
    param([string]$Path, [string]$Name, $Value, [string]$Type = "DWord")
    if (!(Test-Path $Path)) {
        New-Item -Path $Path -Force | Out-Null
    }
    Set-ItemProperty -Path $Path -Name $Name -Value $Value -Type $Type -Force | Out-Null
}

# Montage de la ruche NTUSER.DAT du profil Default User pour que tous les comptes héritent des réglages
$DefaultUserDat = "C:\Users\Default\NTUSER.DAT"
$HiveMounted = $false
if (Test-Path $DefaultUserDat) {
    reg.exe load "HKU\NoosDefault" $DefaultUserDat | Out-Null
    New-PSDrive -Name HKUD -PSProvider Registry -Root "HKEY_USERS\NoosDefault" | Out-Null
    $HiveMounted = $true
}
"#,
    );

    // 1. Copilot
    if cfg.has_feature("remove_copilot") {
        ps.push_str(
            r#"
# --- Suppression et blocage de Microsoft Copilot ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsCopilot" "TurnOffWindowsCopilot" 1
Set-RegValue "HKCU:\Software\Policies\Microsoft\Windows\WindowsCopilot" "TurnOffWindowsCopilot" 1
Set-RegValue "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "ShowCopilotButton" 0
if ($HiveMounted) {
    Set-RegValue "HKUD:\Software\Policies\Microsoft\Windows\WindowsCopilot" "TurnOffWindowsCopilot" 1
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "ShowCopilotButton" 0
}
Get-AppxProvisionedPackage -Online | Where-Object { $_.DisplayName -match "Copilot" } | Remove-AppxProvisionedPackage -Online | Out-Null
Get-AppxPackage -AllUsers "*Copilot*" | Remove-AppxPackage -AllUsers | Out-Null
"#,
        );
    }

    // 2. Windows Recall / AI
    if cfg.has_feature("disable_windows_recall") {
        ps.push_str(
            r#"
# --- Désactivation de Windows Recall & Analyse IA ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsAI" "DisableAIDataAnalysis" 1
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsAI" "TurnOffSavingSnapshots" 1
if ($HiveMounted) {
    Set-RegValue "HKUD:\Software\Policies\Microsoft\Windows\WindowsAI" "DisableAIDataAnalysis" 1
}
DISM.exe /Online /Disable-Feature /FeatureName:Recall /NoRestart | Out-Null
"#,
        );
    }

    // 3. Bing Search
    if cfg.has_feature("disable_bing_search") {
        ps.push_str(
            r#"
# --- Désactivation de Bing et de la recherche Web dans le menu Démarrer ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Windows Search" "DisableWebSearch" 1
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Windows Search" "ConnectedSearchUseWeb" 0
Set-RegValue "HKCU:\Software\Policies\Microsoft\Windows\Explorer" "DisableSearchBoxSuggestions" 1
Set-RegValue "HKCU:\Software\Microsoft\Windows\CurrentVersion\Search" "BingSearchEnabled" 0
if ($HiveMounted) {
    Set-RegValue "HKUD:\Software\Policies\Microsoft\Windows\Explorer" "DisableSearchBoxSuggestions" 1
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\Search" "BingSearchEnabled" 0
}
"#,
        );
    }

    // 4. Cortana
    if cfg.has_feature("remove_cortana") {
        ps.push_str(
            r#"
# --- Désactivation et suppression de Cortana ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Windows Search" "AllowCortana" 0
Get-AppxProvisionedPackage -Online | Where-Object { $_.DisplayName -match "549981C3F5F10|Cortana" } | Remove-AppxProvisionedPackage -Online | Out-Null
Get-AppxPackage -AllUsers "*549981C3F5F10*" | Remove-AppxPackage -AllUsers | Out-Null
"#,
        );
    }

    // 5. Widgets & News
    if cfg.has_feature("disable_widgets_news") {
        ps.push_str(
            r#"
# --- Suppression des Widgets et Actualités MSN ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Dsh" "AllowNewsAndInterests" 0
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Windows Feeds" "EnableFeeds" 0
Set-RegValue "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "TaskbarDa" 0
if ($HiveMounted) {
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "TaskbarDa" 0
}
"#,
        );
    }

    // 6. Applications grand public (Consumer Bloatware)
    if cfg.has_feature("remove_consumer_apps") {
        ps.push_str(
            r#"
# --- Purge des applications grand public préinstallées (Bloatwares) ---
$BloatPackages = @(
    "Clipchamp.Clipchamp",
    "Microsoft.BingFinance",
    "Microsoft.BingNews",
    "Microsoft.BingSports",
    "Microsoft.BingWeather",
    "Microsoft.GetHelp",
    "Microsoft.Getstarted",
    "Microsoft.Microsoft3DViewer",
    "Microsoft.MicrosoftSolitaireCollection",
    "Microsoft.MixedReality.Portal",
    "Microsoft.People",
    "Microsoft.PowerAutomateDesktop",
    "Microsoft.SkypeApp",
    "Microsoft.Todos",
    "Microsoft.WindowsAlarms",
    "Microsoft.WindowsCommunicationsApps",
    "Microsoft.WindowsFeedbackHub",
    "Microsoft.WindowsMaps",
    "Microsoft.WindowsSoundRecorder",
    "Microsoft.YourPhone",
    "Microsoft.ZuneMusic",
    "Microsoft.ZuneVideo",
    "Disney.37853FC22B2CE",
    "SpotifyAB.SpotifyMusic",
    "BytedancePte.Ltd.TikTok"
)
foreach ($Pkg in $BloatPackages) {
    Get-AppxProvisionedPackage -Online | Where-Object { $_.DisplayName -like "*$Pkg*" } | Remove-AppxProvisionedPackage -Online | Out-Null
    Get-AppxPackage -AllUsers "*$Pkg*" | Remove-AppxPackage -AllUsers | Out-Null
}
"#,
        );
    }

    // 7. Bloquer l'installation silencieuse d'apps sponsorisées
    if cfg.has_feature("block_sponsored_apps") {
        ps.push_str(
            r#"
# --- Blocage du téléchargement silencieux d'applications sponsorisées ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\CloudContent" "DisableWindowsConsumerFeatures" 1
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\CloudContent" "DisableCloudOptimizedContent" 1
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\CloudContent" "DisableSoftLanding" 1
$CdmPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager"
Set-RegValue $CdmPath "SilentInstalledAppsEnabled" 0
Set-RegValue $CdmPath "OemPreInstalledAppsEnabled" 0
Set-RegValue $CdmPath "PreInstalledAppsEnabled" 0
Set-RegValue $CdmPath "ContentDeliveryAllowed" 0
if ($HiveMounted) {
    $DefCdm = "HKUD:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager"
    Set-RegValue $DefCdm "SilentInstalledAppsEnabled" 0
    Set-RegValue $DefCdm "OemPreInstalledAppsEnabled" 0
    Set-RegValue $DefCdm "PreInstalledAppsEnabled" 0
    Set-RegValue $DefCdm "ContentDeliveryAllowed" 0
}
"#,
        );
    }

    // 8. OneDrive
    if cfg.has_feature("remove_onedrive") {
        ps.push_str(
            r#"
# --- Désinstallation et blocage de Microsoft OneDrive ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\OneDrive" "DisableFileSyncNGSC" 1
if ($HiveMounted) {
    Remove-ItemProperty -Path "HKUD:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "OneDriveSetup" -Force
}
Stop-Process -Name "OneDrive*" -Force
$OneDriveSetup64 = "$env:SystemRoot\SysWOW64\OneDriveSetup.exe"
$OneDriveSetup32 = "$env:SystemRoot\System32\OneDriveSetup.exe"
if (Test-Path $OneDriveSetup64) { Start-Process $OneDriveSetup64 -ArgumentList "/uninstall" -Wait -NoNewWindow }
if (Test-Path $OneDriveSetup32) { Start-Process $OneDriveSetup32 -ArgumentList "/uninstall" -Wait -NoNewWindow }
"#,
        );
    }

    // 9. Office Hub, Outlook Web & Teams Personnel
    if cfg.has_feature("remove_office_outlook_hub") {
        ps.push_str(
            r#"
# --- Suppression de Microsoft 365 Hub, Outlook Web et Teams Personnel ---
$OfficeBloat = @("Microsoft.MicrosoftOfficeHub", "Microsoft.OutlookForWindows", "MSTeams", "MicrosoftTeams")
foreach ($Pkg in $OfficeBloat) {
    Get-AppxProvisionedPackage -Online | Where-Object { $_.DisplayName -like "*$Pkg*" } | Remove-AppxProvisionedPackage -Online | Out-Null
    Get-AppxPackage -AllUsers "*$Pkg*" | Remove-AppxPackage -AllUsers | Out-Null
}
Set-RegValue "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Communications" "ConfigureChatAutoInstall" 0
"#,
        );
    }

    // 10. Allégement Microsoft Edge
    if cfg.has_feature("remove_telemetry_edge_bloat") {
        ps.push_str(
            r#"
# --- Allégement de Microsoft Edge ---
$EdgePolicy = "HKLM:\SOFTWARE\Policies\Microsoft\Edge"
Set-RegValue $EdgePolicy "HideFirstRunExperience" 1
Set-RegValue $EdgePolicy "HubsSidebarEnabled" 0
Set-RegValue $EdgePolicy "EdgeShoppingAssistantEnabled" 0
Set-RegValue $EdgePolicy "BackgroundModeEnabled" 0
Set-RegValue $EdgePolicy "StartupBoostEnabled" 0
Set-RegValue $EdgePolicy "ShowRecommendationsEnabled" 0
"#,
        );
    }

    // 11. Xbox & GameOverlay (optionnel)
    if cfg.has_feature("remove_xbox_gamebar") {
        ps.push_str(
            r#"
# --- Suppression des applications Xbox & GameOverlay ---
$XboxPackages = @(
    "Microsoft.GamingApp",
    "Microsoft.XboxApp",
    "Microsoft.XboxGameOverlay",
    "Microsoft.XboxGamingOverlay",
    "Microsoft.XboxIdentityProvider",
    "Microsoft.XboxSpeechToTextOverlay"
)
foreach ($Pkg in $XboxPackages) {
    Get-AppxProvisionedPackage -Online | Where-Object { $_.DisplayName -like "*$Pkg*" } | Remove-AppxProvisionedPackage -Online | Out-Null
    Get-AppxPackage -AllUsers "*$Pkg*" | Remove-AppxPackage -AllUsers | Out-Null
}
"#,
        );
    }

    // 12. Télémétrie & DiagTrack
    if cfg.has_feature("disable_telemetry_services") {
        ps.push_str(
            r#"
# --- Désactivation de la télémétrie Windows et des services DiagTrack / dmwappushservice ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection" "AllowTelemetry" 0
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection" "MaxTelemetryAllowed" 0
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\DataCollection" "DoNotShowFeedbackNotifications" 1
Set-RegValue "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\DataCollection" "AllowTelemetry" 0
foreach ($Svc in @("DiagTrack", "dmwappushservice")) {
    Stop-Service -Name $Svc -Force
    Set-Service -Name $Svc -StartupType Disabled
}
"#,
        );
    }

    // 13. Identifiant publicitaire & suivi d'activité
    if cfg.has_feature("disable_advertising_tracking") {
        ps.push_str(
            r#"
# --- Blocage de l'identifiant publicitaire et du suivi d'activité ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\AdvertisingInfo" "DisabledByGroupPolicy" 1
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\System" "EnableActivityFeed" 0
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\System" "PublishUserActivities" 0
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\System" "UploadUserActivities" 0
Set-RegValue "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "Start_TrackProgs" 0
if ($HiveMounted) {
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\AdvertisingInfo" "Enabled" 0
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "Start_TrackProgs" 0
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager" "RotatingLockScreenOverlayEnabled" 0
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager" "SubscribedContent-338387Enabled" 0
}
"#,
        );
    }

    // 14. Delivery Optimization P2P
    if cfg.has_feature("disable_wi_fi_sense_delivery") {
        ps.push_str(
            r#"
# --- Désactivation du partage P2P Windows Update (Delivery Optimization) ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\DeliveryOptimization" "DODownloadMode" 0
Set-RegValue "HKLM:\SOFTWARE\Microsoft\WcmSvc\wifinetworkmanager\config" "AutoConnectAllowedOEM" 0
"#,
        );
    }

    // 15. Menu contextuel classique Windows 10
    if cfg.has_feature("classic_context_menu") {
        ps.push_str(
            r#"
# --- Restauration du menu contextuel clic-droit classique (Windows 10) ---
New-Item -Path "HKCU:\Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32" -Value "" -Force | Out-Null
if ($HiveMounted) {
    New-Item -Path "HKUD:\Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32" -Value "" -Force | Out-Null
}
"#,
        );
    }

    // 16. Explorateur Pro
    if cfg.has_feature("explorer_power_user") {
        ps.push_str(
            r#"
# --- Explorateur Pro : Afficher les extensions de fichiers & ouvrir sur 'Ce PC' ---
Set-RegValue "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "HideFileExt" 0
Set-RegValue "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "LaunchTo" 1
if ($HiveMounted) {
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "HideFileExt" 0
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "LaunchTo" 1
}
"#,
        );
    }

    // 17. Épurer la barre des tâches et le menu Démarrer
    if cfg.has_feature("clean_taskbar_start") {
        ps.push_str(
            r#"
# --- Épurage de la barre des tâches et du menu Démarrer ---
Set-RegValue "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "ShowTaskViewButton" 0
Set-RegValue "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "TaskbarMn" 0
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Explorer" "HideRecommendedSection" 1
if ($HiveMounted) {
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "ShowTaskViewButton" 0
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced" "TaskbarMn" 0
}
"#,
        );
    }

    // 18. Bloquer les écrans de harcèlement post-installation
    if cfg.has_feature("disable_bing_lockscreen_tips") {
        ps.push_str(
            r#"
# --- Blocage des rappels 'Terminer la configuration de votre appareil' ---
Set-RegValue "HKCU:\Software\Microsoft\Windows\CurrentVersion\UserProfileEngagement" "ScoobeSystemSettingEnabled" 0
if ($HiveMounted) {
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\UserProfileEngagement" "ScoobeSystemSettingEnabled" 0
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager" "SubscribedContent-310093Enabled" 0
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager" "SubscribedContent-338389Enabled" 0
}
"#,
        );
    }

    // 19. Optimisations Performances & Latence
    if cfg.has_feature("high_perf_gaming_tweaks") {
        ps.push_str(
            r#"
# --- Optimisations Performances & arrêt GameDVR ---
Set-RegValue "HKLM:\SOFTWARE\Policies\Microsoft\Windows\GameDVR" "AllowGameDVR" 0
Set-RegValue "HKCU:\System\GameConfigStore" "GameDVR_Enabled" 0
Set-RegValue "HKCU:\Software\Microsoft\Windows\CurrentVersion\GameDVR" "AppCaptureEnabled" 0
if ($HiveMounted) {
    Set-RegValue "HKUD:\System\GameConfigStore" "GameDVR_Enabled" 0
    Set-RegValue "HKUD:\Software\Microsoft\Windows\CurrentVersion\GameDVR" "AppCaptureEnabled" 0
}
powercfg.exe /setactive 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c | Out-Null
"#,
        );
    }

    // Démontage propre de la ruche Default User
    ps.push_str(
        r#"
if ($HiveMounted) {
    Remove-PSDrive -Name HKUD -Force
    [gc]::Collect()
    Start-Sleep -Milliseconds 500
    reg.exe unload "HKU\NoosDefault" | Out-Null
}

Stop-Transcript
"#,
    );

    ps
}

/// Déploie `autounattend.xml` et les scripts `$OEM$` sur le point de montage de la clé USB Windows.
pub fn write_unattend_and_debloat_to_usb(
    usb_mount_dir: &Path,
    cfg: &WindowsCustomConfig,
) -> AppResult<()> {
    if !cfg.enabled {
        return Ok(());
    }

    // 1. Écriture de `/autounattend.xml` à la racine de la clé USB
    let autounattend_content = generate_autounattend_xml(cfg);
    let root_unattend = usb_mount_dir.join("autounattend.xml");
    fs::write(&root_unattend, autounattend_content.as_bytes())?;

    // 2. Création de l'arborescence `$OEM$/$$/Setup/Scripts/` (copiée dans `C:\Windows\Setup\Scripts\`)
    let scripts_dir = usb_mount_dir
        .join("sources")
        .join("$OEM$")
        .join("$$")
        .join("Setup")
        .join("Scripts");
    fs::create_dir_all(&scripts_dir)?;

    let ps_script = generate_debloat_powershell_script(cfg);
    fs::write(scripts_dir.join("noos_debloat.ps1"), ps_script.as_bytes())?;

    let setup_complete_cmd = "@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \"%WINDIR%\\Setup\\Scripts\\noos_debloat.ps1\"\r\n";
    fs::write(
        scripts_dir.join("SetupComplete.cmd"),
        setup_complete_cmd.as_bytes(),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_autounattend_and_debloat() {
        let cfg = WindowsCustomConfig {
            local_username: "Chomiam".to_string(),
            ..Default::default()
        };
        let xml = generate_autounattend_xml(&cfg);
        assert!(xml.contains("<Name>Chomiam</Name>"));
        assert!(xml.contains("BypassNRO"));
        assert!(xml.contains("BypassTPMCheck"));

        let ps = generate_debloat_powershell_script(&cfg);
        assert!(ps.contains("TurnOffWindowsCopilot"));
        assert!(ps.contains("DisableAIDataAnalysis"));
        assert!(ps.contains("AllowTelemetry"));
    }
}
