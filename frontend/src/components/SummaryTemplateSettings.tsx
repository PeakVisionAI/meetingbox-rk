'use client';

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Pencil, Plus, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import type { SummaryTemplateInfo } from '@/hooks/meeting-details/useTemplates';

interface TemplateSection {
  title: string;
  instruction: string;
  format: 'paragraph' | 'list' | 'string';
  item_format?: string;
}

interface TemplateDetails extends SummaryTemplateInfo {
  sections: TemplateSection[];
}

const emptyTemplate = (): TemplateDetails => ({
  id: `custom-${Date.now()}`,
  name: '',
  description: '',
  is_custom: true,
  sections: [
    { title: '会议概览', instruction: '完整概括会议背景、目标和结论', format: 'paragraph' },
    { title: '关键决策', instruction: '列出会议明确作出的决定及依据', format: 'list' },
    { title: '行动项', instruction: '列出负责人、任务、截止时间和转写依据', format: 'list' },
  ],
});

export function SummaryTemplateSettings() {
  const [templates, setTemplates] = useState<SummaryTemplateInfo[]>([]);
  const [editor, setEditor] = useState<TemplateDetails | null>(null);
  const [saving, setSaving] = useState(false);
  const [loading, setLoading] = useState(true);

  const loadTemplates = useCallback(async () => {
    setLoading(true);
    try {
      setTemplates(await invoke<SummaryTemplateInfo[]>('api_list_templates'));
    } catch (error) {
      toast.error('无法读取总结模板', { description: String(error) });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadTemplates();
  }, [loadTemplates]);

  const openTemplate = async (template: SummaryTemplateInfo) => {
    try {
      setEditor(await invoke<TemplateDetails>('api_get_template_details', { templateId: template.id }));
    } catch (error) {
      toast.error('无法打开模板', { description: String(error) });
    }
  };

  const updateSection = (index: number, patch: Partial<TemplateSection>) => {
    setEditor((current) => current ? {
      ...current,
      sections: current.sections.map((section, sectionIndex) => sectionIndex === index ? { ...section, ...patch } : section),
    } : current);
  };

  const saveTemplate = async () => {
    if (!editor?.name.trim() || !editor.description.trim()) {
      toast.error('请填写模板名称和用途说明');
      return;
    }
    if (!editor.sections.length || editor.sections.some((section) => !section.title.trim() || !section.instruction.trim())) {
      toast.error('每个总结栏目都需要名称和提取要求');
      return;
    }
    setSaving(true);
    try {
      await invoke('api_save_custom_template', {
        templateId: editor.id,
        template: {
          name: editor.name.trim(),
          description: editor.description.trim(),
          sections: editor.sections,
        },
      });
      setEditor(null);
      await loadTemplates();
      toast.success('自定义总结模板已保存');
    } catch (error) {
      toast.error('无法保存模板', { description: String(error) });
    } finally {
      setSaving(false);
    }
  };

  const deleteTemplate = async (template: SummaryTemplateInfo) => {
    if (!window.confirm(`确定删除“${template.name}”吗？`)) return;
    try {
      await invoke('api_delete_custom_template', { templateId: template.id });
      await loadTemplates();
      toast.success('自定义模板已删除');
    } catch (error) {
      toast.error('无法删除模板', { description: String(error) });
    }
  };

  return <section className="hyos-template-settings">
    <div className="hyos-settings-section-head"><div><h2>会议总结模板</h2><p>选择内置模板，或按业务场景添加自定义栏目。</p></div><button className="hyos-primary" onClick={() => setEditor(emptyTemplate())}><Plus size={16} />添加模板</button></div>
    <div className="hyos-template-list">
      {loading ? <div className="hyos-empty-files">正在读取模板…</div> : templates.map((template) => <article className="hyos-template-row" key={template.id}><div><b>{template.name}</b>{template.id === 'standard_meeting' && <span>默认</span>}{template.is_custom && <span>自定义</span>}<p>{template.description}</p></div><div className="hyos-template-actions"><button className="hyos-secondary" onClick={() => openTemplate(template)}>{template.is_custom ? <><Pencil size={14} />编辑</> : '预览'}</button>{template.is_custom && <button className="hyos-icon-danger" onClick={() => deleteTemplate(template)} aria-label={`删除${template.name}`} title="删除模板"><Trash2 size={15} /></button>}</div></article>)}
    </div>
    {editor && <div className="hyos-modal-backdrop"><div className="hyos-modal hyos-template-editor" role="dialog" aria-modal="true" aria-labelledby="template-editor-title"><button className="hyos-modal-close" onClick={() => setEditor(null)} aria-label="关闭">×</button><h2 id="template-editor-title">{editor.is_custom ? (editor.name ? '编辑总结模板' : '添加总结模板') : '预览总结模板'}</h2><p>按会议场景定义最终总结需要提取的栏目。</p><div className="hyos-template-fields"><label>模板名称<input value={editor.name} disabled={!editor.is_custom} onChange={(event) => setEditor({ ...editor, name: event.target.value })} /></label><label>用途说明<input value={editor.description} disabled={!editor.is_custom} onChange={(event) => setEditor({ ...editor, description: event.target.value })} /></label></div><div className="hyos-template-editor-head"><b>总结栏目</b>{editor.is_custom && <button className="hyos-secondary" onClick={() => setEditor({ ...editor, sections: [...editor.sections, { title: '', instruction: '', format: 'paragraph' }] })}><Plus size={14} />添加栏目</button>}</div><div className="hyos-template-section-list">{editor.sections.map((section, index) => <div className="hyos-template-section" key={`${editor.id}-${index}`}><span>{index + 1}</span><input value={section.title} disabled={!editor.is_custom} placeholder="栏目名称" onChange={(event) => updateSection(index, { title: event.target.value })} /><input value={section.instruction} disabled={!editor.is_custom} placeholder="需要提取的内容" onChange={(event) => updateSection(index, { instruction: event.target.value })} />{editor.is_custom && <button className="hyos-icon-danger" aria-label="删除栏目" title="删除栏目" disabled={editor.sections.length === 1} onClick={() => setEditor({ ...editor, sections: editor.sections.filter((_, sectionIndex) => sectionIndex !== index) })}><Trash2 size={14} /></button>}</div>)}</div><div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => setEditor(null)}>{editor.is_custom ? '取消' : '关闭'}</button>{editor.is_custom && <button className="hyos-primary" onClick={saveTemplate} disabled={saving}>{saving ? '正在保存…' : '保存模板'}</button>}</div></div></div>}
  </section>;
}
