export type TargetGame = 'GenshinImpact' | 'HonkaiStarRail' | 'Wuwa' | 'ZZZ' | 'HonkaiImpact3rd';
export type GroupType = 'exclusiveSlot' | 'customParallel';
export type ModsPathStatus = 'normal' | 'empty' | 'noAccess' | 'notSet';
export type LayoutMode = 'grid' | 'carousel' | 'automatic';
export type SortingType = 'default' | 'alphabetical' | 'recentMod' | 'reverseAlphabetical';
export type CursorType = 'normal' | 'precision';
export type NotificationLevel = 'info' | 'warning' | 'error' | 'success';
export type KeybindProfile = 'GI' | 'HSR' | 'Wuwa' | 'ZZZ' | 'Hi3';

export interface KeybindData {
  key: string;
  value: string;
  section: string;
  disabled: boolean;
  extension: string;
}

export interface ErroredLines {
  lineNumber: number;
  line: string;
  errorType: number;
}

export interface ModIniData {
  iniPath: string;
  keybinds: KeybindData[];
  keybindCommands: KeybindData[];
  constants: KeybindData[];
  overrides: KeybindData[];
  commandLists: KeybindData[];
  presentSections: string[];
  fileRelativePath: string;
}

export interface ModData {
  modPath: string;
  modName: string;
  modIni: ModIniData | null;
  isActive: boolean;
  isFavorite: boolean;
  isNamespaced: boolean;
  hasNonmanagedModsCrashlineFix: boolean;
  erroredLines: ErroredLines[];
  erroredPreexisting: ErroredLines[];
  missingEndif: string[];
  namespaces: string[];
  knownLibraries: string[];
  duplicateLibraries: [string, string[]][];
  nonexistentLibraries: string[];
  namespaceError: boolean;
  modDisabled: boolean;
  pathTooLong: boolean;
  modIndex: number;
}

export interface ModGroupData {
  groupPath: string;
  groupName: string;
  groupId: number;
  groupIndex: number;
  mods: ModData[];
  modCount: number;
  isActive: boolean;
  isFavorite: boolean;
  groupDisabled: boolean;
  groupType: GroupType;
  hasChild: boolean;
  children: ModGroupData[];
  activeModIndex: number;
}

export interface HotkeyKeyboard {
  mod1: string;
  mod2: string;
  keyNext: string;
  keyPrev: string;
  keyHide: string;
  keySelect: string;
  keyCancel: string;
  keyScrollup: string;
  keyScrolldown: string;
}

export interface HotkeyGamepad {
  dpadUp: boolean;
  dpadDown: boolean;
  dpadLeft: boolean;
  dpadRight: boolean;
  buttonNext: string;
  buttonPrev: string;
  buttonHide: string;
  buttonSelect: string;
  buttonCancel: string;
}

export interface AppSettings {
  targetGame: TargetGame;
  hotkey: HotkeyKeyboard;
  gamepadHotkey: HotkeyGamepad;
  gameModsPath: Record<TargetGame, string>;
  gameProfile: Record<string, KeybindProfile>;
  interfaceScale: number;
  bgTransparency: number;
  dynamicBackground: boolean;
  modGroupingMode: LayoutMode;
  modsSortingType: SortingType;
  reverseSort: boolean;
  cursorType: CursorType;
  language: string;
  darkMode: boolean;
  autoFolderIcon: boolean;
  autoPriorityIndex: boolean;
  checkUpdateOnStart: boolean;
  autoTopWindow: boolean;
  isWindowFullscreen: boolean;
  showKeypressOnScreen: boolean;
  simulateKeyOnSelection: boolean;
  usePreciseHotkey: boolean;
  swapCancelKeybind: boolean;
  alwaysShowMenuOnHotkey: boolean;
  hotkeyOnlyInMigoto: boolean;
  folderIconBlacklist: string[];
  disabledKbInputs: string[];
  disabledGamepadInputs: string[];
  selectedModIndex: Record<string, number>;
  selectedGroupIndex: Record<string, number>;
  enabledKb: boolean;
  enabledGamepad: boolean;
  showErroredMods: boolean;
  showFavoritesOnly: boolean;
  checkNamespaceConflict: boolean;
}

export interface PlatformInfo {
  os: 'windows' | 'linux' | 'macos' | 'unknown';
  desktopSession: string;
  isWayland: boolean;
  isX11: boolean;
  isWslg: boolean;
  transparencySupported: boolean;
  platformDepsStatus?: PlatformDepsStatus;
}

export interface PlatformDepsStatus {
  xdotoolAvailable: boolean;
  ydotoolAvailable: boolean;
  libxtstAvailable: boolean;
  hasAccessibilityPermission: boolean;
}

export interface CloudLink {
  name: string;
  url: string;
  icon?: string;
}

export interface CloudMessage {
  title: string;
  content: string;
  level: 'info' | 'warning' | 'error' | 'success';
  date?: string;
}

export interface CloudData {
  links: CloudLink[];
  messages: CloudMessage[];
  autoIcons: Record<string, string>;
  knownLibraries: Record<string, string[]>;
}

export interface InvokeResult<T> {
  success: boolean;
  data?: T;
  error?: string;
}

export interface UpdateModDataResult {
  groups: ModGroupData[];
  hashConflicts: [string, string][];
  debugLog: string;
  errorMessage?: string;
}
