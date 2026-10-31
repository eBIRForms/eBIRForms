; Inno Setup script for eBIRForms (formerly Buwiz) on Windows.
;   iscc /DAppVersion=0.1.0 /DExePath=..\..\target\release\buwiz.exe
;        /DWebView2Setup=..\..\target\MicrosoftEdgeWebview2Setup.exe buwiz.iss
; The print preview needs the WebView2 Runtime (Evergreen). Windows 11 and
; current Windows 10 have it; otherwise setup runs Microsoft's small
; Evergreen bootstrapper, which downloads and installs it.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef ExePath
  #define ExePath "..\..\target\release\buwiz.exe"
#endif
#ifndef WebView2Setup
  #define WebView2Setup "..\..\target\MicrosoftEdgeWebview2Setup.exe"
#endif

; v0.2.0 renamed the app eBIRForms. AppId and the install folder
; ({autopf}\Buwiz) stay, so 0.1.0 upgrades in place.
[Setup]
AppId={{5B0E7C1A-3D59-4F0B-9C8E-B0F1A2C3D4E5}
AppName=eBIRForms
AppVersion={#AppVersion}
AppPublisher=Buwiz
DefaultDirName={autopf}\Buwiz
DefaultGroupName=eBIRForms
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\target\installer
OutputBaseFilename=eBIRForms-{#AppVersion}-setup-x64
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\buwiz.exe

[Files]
Source: "{#ExePath}"; DestDir: "{app}"; DestName: "buwiz.exe"; Flags: ignoreversion
Source: "{#WebView2Setup}"; DestDir: "{tmp}"; Flags: deleteafterinstall; Check: NeedsWebView2

[Icons]
Name: "{autoprograms}\eBIRForms"; Filename: "{app}\buwiz.exe"
Name: "{autodesktop}\eBIRForms"; Filename: "{app}\buwiz.exe"; Tasks: desktopicon

; 0.1.0's shortcuts, named Buwiz: the renamed ones replace them.
[InstallDelete]
Type: files; Name: "{autoprograms}\Buwiz.lnk"
Type: files; Name: "{autodesktop}\Buwiz.lnk"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Run]
Filename: "{tmp}\MicrosoftEdgeWebview2Setup.exe"; Parameters: "/silent /install"; StatusMsg: "Installing the Microsoft Edge WebView2 Runtime..."; Check: NeedsWebView2; Flags: waituntilterminated
Filename: "{app}\buwiz.exe"; Description: "{cm:LaunchProgram,eBIRForms}"; Flags: nowait postinstall skipifsilent

[Code]
const
  WebView2Key = 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';

function HasVersion(Root: Integer; Key: String): Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(Root, Key, 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0');
end;

// The Evergreen runtime registers per machine (WOW6432Node on 64-bit
// Windows) or per user.
function NeedsWebView2: Boolean;
begin
  Result := not (HasVersion(HKLM32, WebView2Key) or HasVersion(HKLM64, WebView2Key) or HasVersion(HKCU, WebView2Key));
end;
