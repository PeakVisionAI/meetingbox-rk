import React from 'react';
import { Download } from 'lucide-react';
import { toast } from 'sonner';
import { formatDisplayVersion, UpdateInfo } from '@/services/updateService';

let globalShowDialogCallback: (() => void) | null = null;

export function setUpdateDialogCallback(callback: () => void) {
  globalShowDialogCallback = callback;
}

export function showUpdateNotification(updateInfo: UpdateInfo, onUpdateClick?: () => void) {
  const handleClick = () => {
    if (onUpdateClick) {
      onUpdateClick();
    } else if (globalShowDialogCallback) {
      globalShowDialogCallback();
    }
  };

  toast.info(
    <div className="flex items-center justify-between gap-4">
      <div className="flex items-center gap-2">
        <Download className="h-4 w-4" />
        <div>
          <p className="font-medium">发现 AI智能会议助手新版本</p>
          <p className="text-sm text-muted-foreground">{formatDisplayVersion(updateInfo.version || '')} 已发布</p>
        </div>
      </div>
      <button
        onClick={(e) => {
          e.stopPropagation();
          handleClick();
        }}
        className="text-sm font-medium text-blue-600 hover:text-blue-700 underline"
      >
        查看详情
      </button>
    </div>,
    {
      duration: 10000,
      position: 'bottom-center',
    }
  );
}
