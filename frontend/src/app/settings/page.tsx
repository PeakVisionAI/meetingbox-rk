'use client';

import { useState } from 'react';
import { ArrowLeft, Cpu, Database, Monitor, KeyRound, Mic, ShieldAlert } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { toast } from 'sonner';
import { useConfig } from '@/contexts/ConfigContext';
import { accessControlService } from '@/services/accessControlService';

export default function SettingsPage() {
  const router = useRouter();
  const { transcriptModelConfig, selectedDevices, storageLocations } = useConfig();
  const [passwordOpen, setPasswordOpen] = useState(false);
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const changePassword = async () => {
    if (newPassword !== confirmPassword) return toast.error('两次输入的新密码不一致');
    if (newPassword.trim().length < 8) return toast.error('新密码至少需要 8 个字符');
    try { await accessControlService.changeAdminPassword(currentPassword, newPassword); setPasswordOpen(false); setCurrentPassword(''); setNewPassword(''); setConfirmPassword(''); toast.success('管理员密码已修改'); }
    catch (error) { toast.error('修改失败', { description: String(error) }); }
  };
  return <div className="hyos-page hyos-settings-page"><div className="hyos-page-title"><div><button className="hyos-back" onClick={() => router.push('/')}><ArrowLeft size={16} />返回工作台</button><h1>设置</h1><p>查看设备状态、管理员账户与系统版本。</p></div></div><section className="hyos-settings-layout"><div className="hyos-panel hyos-settings-panel"><div className="hyos-panel-head"><h2>设备状态</h2><span className="hyos-ok"><i />本地运行</span></div><div className="hyos-setting-row"><Cpu size={18} /><span>本地模型服务<small>{transcriptModelConfig.provider} / {transcriptModelConfig.model}</small></span><b>已配置</b></div><div className="hyos-setting-row"><Monitor size={18} /><span>HDMI / Type-C 外接设备<small>连接状态由操作系统显示设置管理</small></span><b>系统管理</b></div><div className="hyos-setting-row"><Mic size={18} /><span>会议麦克风<small>{selectedDevices.micDevice || '系统默认设备'}</small></span><b>已选择</b></div><div className="hyos-setting-row"><Database size={18} /><span>本地存储<small>{storageLocations?.recordings || '设备本地录音目录'}</small></span><b>本地保存</b></div></div><div className="hyos-panel hyos-settings-panel"><div className="hyos-panel-head"><h2>账户与版本</h2></div><div className="hyos-account"><span>管</span><div><b>管理员</b><small>admin · 唯一管理员账户</small></div><button className="hyos-secondary" onClick={() => setPasswordOpen(true)}><KeyRound size={15} />修改密码</button></div><div className="hyos-warning hyos-settings-warning"><ShieldAlert size={18} /><span><b>请妥善保管管理员密码</b><small>管理员密码无法找回。遗忘后需刷写设备，已有数据可能丢失。</small></span></div><div className="hyos-setting-row"><span>系统版本<small>峰瞰密会智盒桌面应用</small></span><b>HYOS 1.0</b></div><div className="hyos-form-tip">系统版本维护由服务人员现场完成，本页面不提供联网更新或安装包上传。</div></div></section>{passwordOpen && <div className="hyos-modal-backdrop"><div className="hyos-modal compact" role="dialog" aria-modal="true" aria-labelledby="change-password-title"><button className="hyos-modal-close" onClick={() => setPasswordOpen(false)} aria-label="关闭">×</button><h2 id="change-password-title">修改管理员密码</h2><p>修改后请立即妥善保存，新密码无法找回。</p><label>当前密码<input type="password" value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} /></label><label>新密码<input type="password" value={newPassword} onChange={(event) => setNewPassword(event.target.value)} /></label><label>再次输入新密码<input type="password" value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value)} /></label><div className="hyos-form-tip">新密码至少 8 个字符。</div><div className="hyos-warning"><b>管理员密码不可找回</b><span>遗忘后需由服务人员刷写设备，已有数据可能丢失。</span></div><div className="hyos-modal-actions"><button className="hyos-secondary" onClick={() => setPasswordOpen(false)}>取消</button><button className="hyos-primary" onClick={changePassword} disabled={!currentPassword || !newPassword || !confirmPassword}>确认修改</button></div></div></div>}</div>;
}
