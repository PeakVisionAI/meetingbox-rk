let pendingMeetingPassword: string | null = null;

export const pendingMeetingAccess = {
  set(password: string | null) {
    pendingMeetingPassword = password?.trim() || null;
  },
  get() {
    return pendingMeetingPassword;
  },
  clear() {
    pendingMeetingPassword = null;
  },
};
