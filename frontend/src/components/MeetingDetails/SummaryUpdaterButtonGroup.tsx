"use client";

import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Copy, Save, Loader2, Search, FolderOpen } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { uiText } from '@/lib/uiText';

interface SummaryUpdaterButtonGroupProps {
  isSaving: boolean;
  isDirty: boolean;
  onSave: () => Promise<void>;
  onCopy: () => Promise<void>;
  onFind?: () => void;
  onOpenFolder: () => Promise<void>;
  hasSummary: boolean;
}

export function SummaryUpdaterButtonGroup({
  isSaving,
  isDirty,
  onSave,
  onCopy,
  onFind,
  onOpenFolder,
  hasSummary
}: SummaryUpdaterButtonGroupProps) {
  return (
    <ButtonGroup>
      {/* Save button */}
      <Button
        variant="outline"
        size="sm"
        className={`${isDirty ? 'bg-green-200' : ""}`}
        title={isSaving ? uiText.saving : uiText.saveChanges}
        onClick={() => {
          Analytics.trackButtonClick('save_changes', 'meeting_details');
          onSave();
        }}
        disabled={isSaving}
      >
        {isSaving ? (
          <>
            <Loader2 className="animate-spin" />
            <span className="hidden lg:inline">{uiText.saving}</span>
          </>
        ) : (
          <>
            <Save />
            <span className="hidden lg:inline">{uiText.save}</span>
          </>
        )}
      </Button>

      {/* Copy button */}
      <Button
        variant="outline"
        size="sm"
        title={uiText.copySummary}
        onClick={() => {
          Analytics.trackButtonClick('copy_summary', 'meeting_details');
          onCopy();
        }}
        disabled={!hasSummary}
        className="cursor-pointer"
      >
        <Copy />
        <span className="hidden lg:inline">{uiText.copy}</span>
      </Button>

      {/* Find button */}
      {/* {onFind && (
        <Button
          variant="outline"
          size="sm"
          title="Find in Summary"
          onClick={() => {
            Analytics.trackButtonClick('find_in_summary', 'meeting_details');
            onFind();
          }}
          disabled={!hasSummary}
          className="cursor-pointer"
        >
          <Search />
          <span className="hidden lg:inline">Find</span>
        </Button>
      )} */}
    </ButtonGroup>
  );
}
