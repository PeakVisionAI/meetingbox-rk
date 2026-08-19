'use client';

import { useCallback, useEffect, useState } from 'react';
import { Plus, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { Hotword, productivityService } from '@/services/productivityService';

export function HotwordSettings() {
  const [items, setItems] = useState<Hotword[]>([]);
  const [phrase, setPhrase] = useState('');
  const [replacement, setReplacement] = useState('');
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setItems(await productivityService.listHotwords());
    } catch (error) {
      toast.error('无法读取热词表', { description: String(error) });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const add = async () => {
    if (!phrase.trim()) return;
    setSaving(true);
    try {
      await productivityService.saveHotword(phrase.trim(), replacement.trim(), true);
      setPhrase('');
      setReplacement('');
      await load();
      toast.success('热词已添加，将在下一场会议生效');
    } catch (error) {
      toast.error('无法保存热词', { description: String(error) });
    } finally {
      setSaving(false);
    }
  };

  const toggle = async (item: Hotword) => {
    try {
      await productivityService.saveHotword(item.phrase, item.replacement ?? '', !item.enabled, item.id);
      await load();
    } catch (error) {
      toast.error('无法更新热词', { description: String(error) });
    }
  };

  const remove = async (item: Hotword) => {
    try {
      await productivityService.deleteHotword(item.id);
      await load();
    } catch (error) {
      toast.error('无法删除热词', { description: String(error) });
    }
  };

  return (
    <section className="hyos-hotword-settings">
      <div className="hyos-settings-section-head">
        <div><h2>热词与领域词表</h2><p>目标词会发送给本机 ASR；填写常见误识别写法后，还会在本地自动纠正。</p></div>
      </div>
      <div className="hyos-hotword-form">
        <label>目标词<input value={phrase} onChange={(event) => setPhrase(event.target.value)} placeholder="例如：峰瞰密会智盒" /></label>
        <label>常见误识别写法<input value={replacement} onChange={(event) => setReplacement(event.target.value)} placeholder="可选，例如：峰看密会智盒" /></label>
        <button className="hyos-primary" onClick={add} disabled={saving || !phrase.trim()}><Plus size={15} />添加</button>
      </div>
      <div className="hyos-form-tip">词表在开始会议时加载；修改后从下一场会议开始生效。</div>
      <div className="hyos-hotword-list">
        {loading ? <p>正在读取词表…</p> : items.length === 0 ? <p>尚未添加热词。</p> : items.map(item => (
          <div className="hyos-hotword-row" key={item.id}>
            <label className="hyos-switch-row"><input type="checkbox" checked={item.enabled} onChange={() => void toggle(item)} /><span>{item.enabled ? '已启用' : '已停用'}</span></label>
            <div><b>{item.phrase}</b><small>{item.replacement ? `纠正：${item.replacement} → ${item.phrase}` : '仅作为领域词发送给 ASR'}</small></div>
            <button className="hyos-icon-danger" onClick={() => void remove(item)} title="删除热词" aria-label={`删除热词 ${item.phrase}`}><Trash2 size={15} /></button>
          </div>
        ))}
      </div>
    </section>
  );
}
