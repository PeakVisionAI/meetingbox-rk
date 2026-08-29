/**
 * Update Service
 *
 * Handles automatic software updates using Tauri updater plugin.
 * Provides update checking, downloading, and installation functionality.
 */

import { check, Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { getVersion } from '@tauri-apps/api/app';

export interface UpdateInfo {
  available: boolean;
  currentVersion: string;
  version?: string;
  date?: string;
  body?: string;
  downloadUrl?: string;
}

export interface UpdateProgress {
  downloaded: number;
  total: number;
  percentage: number;
}

/** The first HYOS release. Tauri provides the packaged value at runtime; this
 * fallback keeps the settings page usable in the browser preview. */
export const FALLBACK_VERSION = '0.1.0';

export function formatDisplayVersion(version: string): string {
  const normalized = version.trim().replace(/^v/i, '');
  const parts = normalized.split('.');
  return parts.length >= 2 ? `v${parts[0]}.${parts[1]}` : `v${normalized}`;
}

/**
 * Update Service
 * Singleton service for managing app updates
 */
export class UpdateService {
  private updateCheckInProgress = false;
  private lastCheckTime: number | null = null;
  private readonly CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;

  /**
   * Check for available updates
   * @param force Force check even if recently checked
   * @returns Promise with update information
   */
  async checkForUpdates(force = false): Promise<UpdateInfo> {
    if (this.updateCheckInProgress) throw new Error('Update check already in progress');
    if (!force && this.wasCheckedRecently()) {
      return { available: false, currentVersion: await this.getCurrentVersion() };
    }
    this.updateCheckInProgress = true;
    this.lastCheckTime = Date.now();
    try {
      const currentVersion = await this.getCurrentVersion();
      const update = await check();
      return update?.available
        ? { available: true, currentVersion, version: update.version, date: update.date, body: update.body }
        : { available: false, currentVersion };
    } finally {
      this.updateCheckInProgress = false;
    }
  }

  /**
   * Download and install the available update
   * @param update The update object from checkForUpdates
   * @param onProgress Optional progress callback
   * @returns Promise that resolves when download completes
   */
  async downloadAndInstall(
    update: Update,
    onProgress?: (progress: UpdateProgress) => void
  ): Promise<void> {
    await update.download();
    onProgress?.({ downloaded: 100, total: 100, percentage: 100 });
    await update.install();
    await relaunch();
  }

  /**
   * Get the current app version
   * @returns Promise with version string
   */
  async getCurrentVersion(): Promise<string> {
    try {
      return await getVersion();
    } catch {
      return FALLBACK_VERSION;
    }
  }

  /**
   * Check if an update check was performed recently
   * @returns true if checked within the interval
   */
  wasCheckedRecently(): boolean {
    return this.lastCheckTime !== null && Date.now() - this.lastCheckTime < this.CHECK_INTERVAL_MS;
  }
}

// Export singleton instance
export const updateService = new UpdateService();
