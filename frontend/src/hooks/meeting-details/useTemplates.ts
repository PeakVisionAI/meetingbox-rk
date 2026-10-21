import { useState, useEffect, useCallback } from 'react';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import { uiText } from '@/lib/uiText';

export interface SummaryTemplateInfo {
  id: string;
  name: string;
  description: string;
  is_custom: boolean;
}

export function useTemplates(initialTemplate = 'standard_meeting') {
  const [availableTemplates, setAvailableTemplates] = useState<Array<{
    id: string;
    name: string;
    description: string;
    is_custom: boolean;
  }>>([]);
  const [selectedTemplate, setSelectedTemplate] = useState<string>(initialTemplate);

  // Fetch available templates on mount
  useEffect(() => {
    const fetchTemplates = async () => {
      try {
        const templates = await invokeTauri('api_list_templates') as Array<{
          id: string;
          name: string;
          description: string;
          is_custom: boolean;
        }>;
        console.log('Available templates:', templates);
        setAvailableTemplates(templates);
        setSelectedTemplate(templates.some((template) => template.id === initialTemplate) ? initialTemplate : 'standard_meeting');
      } catch (error) {
        console.error('Failed to fetch templates:', error);
      }
    };
    fetchTemplates();
  }, [initialTemplate]);

  // Handle template selection
  const handleTemplateSelection = useCallback((templateId: string, templateName: string) => {
    setSelectedTemplate(templateId);
    toast.success(uiText.templateSelected, {
      description: uiText.usingTemplate(templateName),
    });
    Analytics.trackFeatureUsed('template_selected');
  }, []);

  return {
    availableTemplates,
    selectedTemplate,
    handleTemplateSelection,
  };
}
