import { invoke } from '@tauri-apps/api/core';

export const accessControlService = {
  getAdminStatus: () => invoke<{ initialized: boolean }>('get_admin_status'),
  initializeAdminPassword: (initialPassword: string, newPassword: string) => invoke<void>('initialize_admin_password', { initialPassword, newPassword }),
  verifyAdminPassword: (password: string) => invoke<boolean>('verify_admin_password', { password }),
  changeAdminPassword: (currentPassword: string, newPassword: string) => invoke<void>('change_admin_password', { currentPassword, newPassword }),
  setMeetingPassword: (meetingId: string, password: string | null) => invoke<void>('set_meeting_password', { meetingId, password }),
  verifyMeetingPassword: (meetingId: string, password: string) => invoke<boolean>('verify_meeting_password', { meetingId, password }),
  resetMeetingPassword: (meetingId: string, adminPassword: string, newPassword: string) => invoke<void>('reset_meeting_password', { meetingId, adminPassword, newPassword }),
  exportMeetingText: (meetingId: string) => invoke<string>('export_meeting_text', { meetingId }),
};
