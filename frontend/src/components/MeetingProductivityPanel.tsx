'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import { CheckSquare, Download, RefreshCw, UserRoundCog, Users } from 'lucide-react';
import { toast } from 'sonner';
import { Transcript } from '@/types';
import { MeetingAction, SpeakerContact, SpeakerLabel, productivityService } from '@/services/productivityService';

type PanelTab = 'actions' | 'speakers' | 'export';

interface MeetingProductivityPanelProps {
  meetingId: string;
  transcripts: Transcript[];
  open: boolean;
  onClose: () => void;
  onLabelsChange: (labels: Record<string, string>) => void;
  onTranscriptsChanged?: () => Promise<void>;
}

function speakerFromTranscript(transcript: Transcript): string | undefined {
  if (transcript.speaker) return transcript.speaker;
  return transcript.text.match(/^((?:候选发言人|发言人)[^：:\s]{1,20})[：:]/)?.[1];
}

export function MeetingProductivityPanel({ meetingId, transcripts, open, onClose, onLabelsChange, onTranscriptsChanged }: MeetingProductivityPanelProps) {
  const [tab, setTab] = useState<PanelTab>('actions');
  const [actions, setActions] = useState<MeetingAction[]>([]);
  const [labels, setLabels] = useState<SpeakerLabel[]>([]);
  const [contacts, setContacts] = useState<SpeakerContact[]>([]);
  const [busy, setBusy] = useState(false);
  const [newContact, setNewContact] = useState('');
  const [mergeFrom, setMergeFrom] = useState('');
  const [mergeTo, setMergeTo] = useState('');

  const speakerKeys = useMemo(() => Array.from(new Set([...transcripts.map(speakerFromTranscript).filter((value): value is string => Boolean(value)), ...labels.map(label => label.speaker_key)])).sort(), [transcripts, labels]);
  const labelMap = useMemo(() => Object.fromEntries(labels.map(label => [label.speaker_key, label.display_name])), [labels]);

  const load = useCallback(async () => {
    if (!open) return;
    setBusy(true);
    try {
      const [nextActions, nextLabels] = await Promise.all([
        productivityService.listMeetingActions(meetingId),
        productivityService.listSpeakerLabels(meetingId),
      ]);
      setActions(nextActions);
      setLabels(nextLabels);
      onLabelsChange(Object.fromEntries(nextLabels.map(label => [label.speaker_key, label.display_name])));
      try { setContacts(await productivityService.listSpeakerContacts()); } catch { setContacts([]); }
    } catch (error) {
      toast.error('无法读取会议工具数据', { description: String(error) });
    } finally {
      setBusy(false);
    }
  }, [meetingId, onLabelsChange, open]);

  useEffect(() => { void load(); }, [load]);

  const extractActions = async () => {
    setBusy(true);
    try {
      setActions(await productivityService.extractMeetingActions(meetingId));
      toast.success('已从转写中识别候选行动项');
    } catch (error) {
      toast.error('行动项提取失败', { description: String(error) });
    } finally { setBusy(false); }
  };

  const updateAction = async (action: MeetingAction, patch: Partial<MeetingAction>) => {
    const updated = { ...action, ...patch };
    setActions(current => current.map(item => item.id === action.id ? updated : item));
    try { await productivityService.updateMeetingAction(updated); }
    catch (error) { toast.error('行动项保存失败', { description: String(error) }); await load(); }
  };

  const saveSpeaker = async (speakerKey: string, displayName: string, contactId?: string | null) => {
    try {
      await productivityService.saveSpeakerLabel(meetingId, speakerKey, displayName, contactId);
      await load();
      toast.success('说话人名称已保存');
    } catch (error) { toast.error('无法保存说话人', { description: String(error) }); }
  };

  const mergeSpeakers = async () => {
    if (!mergeFrom || !mergeTo || mergeFrom === mergeTo) return;
    setBusy(true);
    try {
      await productivityService.mergeSpeakerLabels(meetingId, mergeFrom, mergeTo);
      setMergeFrom(''); setMergeTo('');
      await onTranscriptsChanged?.();
      await load();
      toast.success('说话人已合并');
    } catch (error) { toast.error('无法合并说话人', { description: String(error) }); }
    finally { setBusy(false); }
  };

  const addContact = async () => {
    if (!newContact.trim()) return;
    try {
      await productivityService.saveSpeakerContact(newContact.trim());
      setNewContact('');
      setContacts(await productivityService.listSpeakerContacts());
      toast.success('声纹联系人已添加');
    } catch (error) { toast.error('需要管理员登录后才能维护联系人', { description: String(error) }); }
  };

  const exportMeeting = async (format: 'md' | 'docx' | 'pdf' | 'srt') => {
    try {
      const path = await productivityService.exportMeeting(meetingId, format);
      toast.success('会议文件已导出', { description: path });
    } catch (error) { toast.error('导出失败', { description: String(error) }); }
  };

  if (!open) return null;
  return (
    <div className="hyos-modal-backdrop">
      <div className="hyos-modal hyos-productivity-modal" role="dialog" aria-modal="true" aria-labelledby="meeting-tools-title">
        <button className="hyos-modal-close" onClick={onClose} aria-label="关闭">×</button>
        <h2 id="meeting-tools-title">会议工具</h2>
        <p>整理说话人、提取行动项并导出可交付的会议文件。</p>
        <div className="hyos-tool-tabs" role="tablist">
          <button className={tab === 'actions' ? 'active' : ''} onClick={() => setTab('actions')}><CheckSquare size={15} />行动项</button>
          <button className={tab === 'speakers' ? 'active' : ''} onClick={() => setTab('speakers')}><Users size={15} />说话人</button>
          <button className={tab === 'export' ? 'active' : ''} onClick={() => setTab('export')}><Download size={15} />导出</button>
        </div>
        <div className="hyos-tool-content">
          {tab === 'actions' && <section><div className="hyos-tool-section-head"><div><b>行动项</b><small>从转写中识别带有“需要、负责、跟进、下一步”等表达的候选项。</small></div><button className="hyos-secondary" onClick={extractActions} disabled={busy}><RefreshCw size={14} />重新提取</button></div><div className="hyos-action-list">{actions.length === 0 ? <p className="hyos-tool-empty">尚未识别行动项。</p> : actions.map(action => <div className="hyos-action-row" key={action.id}><input type="checkbox" checked={action.status === 'done'} onChange={event => void updateAction(action, { status: event.target.checked ? 'done' : 'open' })} /><input className="hyos-action-content" value={action.content} onChange={event => setActions(current => current.map(item => item.id === action.id ? { ...item, content: event.target.value } : item))} onBlur={event => void updateAction(action, { content: event.target.value })} /><input value={action.assignee ?? ''} onChange={event => setActions(current => current.map(item => item.id === action.id ? { ...item, assignee: event.target.value } : item))} onBlur={event => void updateAction(action, { assignee: event.target.value })} placeholder="负责人" /><input type="date" value={action.due_date ?? ''} onChange={event => void updateAction(action, { due_date: event.target.value })} /></div>)}</div></section>}
          {tab === 'speakers' && <section><div className="hyos-contact-create"><input value={newContact} onChange={event => setNewContact(event.target.value)} placeholder="新增声纹联系人姓名" /><button className="hyos-secondary" onClick={addContact}><UserRoundCog size={14} />添加联系人</button></div><div className="hyos-speaker-list">{speakerKeys.length === 0 ? <p className="hyos-tool-empty">当前转写没有可识别的说话人标签。</p> : speakerKeys.map(key => <SpeakerRow key={key} speakerKey={key} initialName={labelMap[key] ?? key} contacts={contacts} label={labels.find(item => item.speaker_key === key)} onSave={saveSpeaker} />)}</div>{speakerKeys.length > 1 && <div className="hyos-speaker-merge"><select value={mergeFrom} onChange={event => setMergeFrom(event.target.value)}><option value="">源说话人</option>{speakerKeys.map(key => <option key={key} value={key}>{labelMap[key] ?? key}</option>)}</select><span>合并到</span><select value={mergeTo} onChange={event => setMergeTo(event.target.value)}><option value="">目标说话人</option>{speakerKeys.map(key => <option key={key} value={key}>{labelMap[key] ?? key}</option>)}</select><button className="hyos-primary" disabled={!mergeFrom || !mergeTo || mergeFrom === mergeTo || busy} onClick={mergeSpeakers}>合并</button></div>}<div className="hyos-form-tip">联系人用于跨会议统一姓名；当前设备的声纹服务未提供注册接口时，不会自动跨会议识别人名。</div></section>}
          {tab === 'export' && <section><div className="hyos-export-grid"><button onClick={() => void exportMeeting('md')}><b>Markdown</b><small>适合继续编辑与知识库归档</small></button><button onClick={() => void exportMeeting('docx')}><b>DOCX</b><small>适合办公文档流转</small></button><button onClick={() => void exportMeeting('pdf')}><b>PDF</b><small>适合固定版式分享</small></button><button onClick={() => void exportMeeting('srt')}><b>SRT 字幕</b><small>保留逐段音频时间轴</small></button></div></section>}
        </div>
      </div>
    </div>
  );
}

function SpeakerRow({ speakerKey, initialName, contacts, label, onSave }: { speakerKey: string; initialName: string; contacts: SpeakerContact[]; label?: SpeakerLabel; onSave: (key: string, name: string, contactId?: string | null) => Promise<void> }) {
  const [name, setName] = useState(initialName);
  const [contactId, setContactId] = useState(label?.contact_id ?? '');
  useEffect(() => { setName(initialName); setContactId(label?.contact_id ?? ''); }, [initialName, label?.contact_id]);
  return <div className="hyos-speaker-row"><span>{speakerKey}</span><input value={name} onChange={event => setName(event.target.value)} aria-label={`${speakerKey} 显示名称`} /><select value={contactId} onChange={event => { const id = event.target.value; setContactId(id); const contact = contacts.find(item => item.id === id); if (contact) setName(contact.name); }}><option value="">不关联联系人</option>{contacts.map(contact => <option key={contact.id} value={contact.id}>{contact.name}</option>)}</select><button className="hyos-secondary" disabled={!name.trim()} onClick={() => void onSave(speakerKey, name.trim(), contactId || null)}>保存</button></div>;
}
