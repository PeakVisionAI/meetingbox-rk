'use client';

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

/**
 * Capabilities of the current deployment, resolved by the Rust platform profile.
 * One UI bundle serves both the RK device and the Intel two-machine setup; the
 * profile decides whether knowledge centre / power controls / readiness gates exist.
 */
export interface PlatformProfile {
  kind: 'intel' | 'rk' | 'dev';
  label: string;
  knowledgeCenter: boolean;
  powerActions: boolean;
  deviceStrip: boolean;
  serviceReadiness: boolean;
  asrStreaming: boolean;
  asrBackend: string;
  summaryModel: string;
}

const FALLBACK: PlatformProfile = {
  kind: 'dev',
  label: '开发机',
  knowledgeCenter: false,
  powerActions: false,
  deviceStrip: false,
  serviceReadiness: false,
  asrStreaming: false,
  asrBackend: 'rk',
  summaryModel: '',
};

export function usePlatformProfile(): PlatformProfile {
  const [profile, setProfile] = useState<PlatformProfile>(FALLBACK);

  useEffect(() => {
    let active = true;
    // Browser dev sessions have no Tauri bridge: keep the conservative fallback.
    invoke<Partial<PlatformProfile>>('get_platform_profile')
      .then((value) => {
        if (active) setProfile({ ...FALLBACK, ...value });
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);

  return profile;
}
