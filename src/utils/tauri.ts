import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'

export async function selectFolder(defaultPath?: string): Promise<string | null> {
  return open({
    directory: true,
    multiple: false,
    defaultPath,
    title: '选择模组路径',
  }) as Promise<string | null>
}

export async function getSettings(): Promise<any> {
  return invoke('get_settings')
}

export async function saveSettings(settings: any): Promise<void> {
  return invoke('save_settings', { settings })
}

export async function resetSettings(): Promise<any> {
  return invoke('reset_settings')
}

export async function getMods(game: string, modsPath: string): Promise<any> {
  return invoke('get_mods', { game, modsPath })
}

export async function refreshMods(game: string, modsPath: string): Promise<any> {
  return invoke('refresh_mods', { game, modsPath })
}

export async function selectMod(game: string, modsPath: string, groupIndex: number, modIndex: number): Promise<any> {
  return invoke('select_mod', { game, modsPath, groupIndex, modIndex })
}

export async function deselectGroupMod(game: string, modsPath: string, groupIndex: number): Promise<any> {
  return invoke('deselect_group_mod', { game, modsPath, groupIndex })
}

export async function addGroup(modsPath: string, game: string, groupName?: string): Promise<any> {
  return invoke('add_group', { modsPath, game, groupName })
}

export async function removeGroup(groupPath: string): Promise<void> {
  return invoke('remove_group', { groupPath })
}

export async function removeMod(modPath: string): Promise<void> {
  return invoke('remove_mod', { modPath })
}

export async function renameMod(modPath: string, newName: string): Promise<string> {
  return invoke('rename_mod', { modPath, newName })
}

export async function renameGroup(groupPath: string, newName: string): Promise<string> {
  return invoke('rename_group', { groupPath, newName })
}

export async function toggleModDisabled(modPath: string, enable: boolean): Promise<void> {
  return invoke('toggle_mod_disabled', { modPath, enable })
}

export async function toggleFavorite(modPath: string): Promise<boolean> {
  return invoke('toggle_favorite', { modPath })
}

export async function isFavorite(modPath: string): Promise<boolean> {
  return invoke('is_favorite', { modPath })
}

export async function openModFolder(modPath: string): Promise<void> {
  return invoke('open_mod_folder', { modPath })
}

export async function openGroupFolder(groupPath: string): Promise<void> {
  return invoke('open_group_folder', { groupPath })
}

export async function restoreAllInis(modsPath: string): Promise<any> {
  return invoke('restore_all_inis', { modsPath })
}

export async function checkModsPathStatus(game: string, modsPath: string): Promise<string> {
  return invoke('check_mods_path_status', { game, modsPath })
}

export async function checkKeypressSupport(): Promise<void> {
  return invoke('check_keypress_support')
}

export async function isGameForeground(game: string): Promise<boolean> {
  return invoke('is_game_foreground', { game })
}

export async function getCursorPosition(): Promise<[number, number]> {
  return invoke('get_cursor_position')
}

export async function simulateSelectGroup(): Promise<void> {
  return invoke('simulate_select_group')
}

export async function simulateSelectMod(): Promise<void> {
  return invoke('simulate_select_mod')
}

export async function getPlatformInfo(): Promise<any> {
  return invoke('get_platform_info')
}

export async function showWindow(windowName: string): Promise<void> {
  return invoke('show_window', { windowName })
}

export async function hideWindow(windowName: string): Promise<void> {
  return invoke('hide_window', { windowName })
}

export async function closeWindow(windowName: string): Promise<void> {
  return invoke('close_window', { windowName })
}

export async function minimizeWindow(windowName: string): Promise<void> {
  return invoke('minimize_window', { windowName })
}

export async function toggleMaximize(windowName: string): Promise<void> {
  return invoke('toggle_maximize', { windowName })
}

export async function reregisterHotkeys(): Promise<void> {
  return invoke('reregister_hotkeys')
}

export async function unregisterHotkeys(): Promise<void> {
  return invoke('unregister_hotkeys')
}

export async function startFileWatcher(modsPath: string): Promise<void> {
  return invoke('start_file_watcher', { modsPath })
}

export async function stopFileWatcher(): Promise<void> {
  return invoke('stop_file_watcher')
}

export async function isSupportedArchive(path: string): Promise<boolean> {
  return invoke('is_supported_archive_cmd', { path })
}

export async function importModAuto(archivePath: string, modsPath: string, password?: string): Promise<any> {
  return invoke('import_mod_auto_cmd', { archivePath, modsPath, password })
}

export async function checkForUpdates(): Promise<any> {
  return invoke('check_for_updates')
}

export async function getAppVersion(): Promise<string> {
  return invoke('get_app_version')
}

export async function compareVersions(current: string, latest: string): Promise<boolean> {
  return invoke('compare_versions', { current, latest })
}
