'use client';

import { useEffect, useState } from 'react';
import { ShieldCheck } from 'lucide-react';
import { toast } from 'sonner';
import { accessControlService } from '@/services/accessControlService';

export function HYOSAccessGate({ children }: { children: React.ReactNode }) {
  const [loading, setLoading] = useState(true);
  const [initialized, setInitialized] = useState(true);
  const [authenticated, setAuthenticated] = useState(false);
  const [password, setPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');

  useEffect(() => {
    if (process.env.NODE_ENV === 'development' && !(window as any).__TAURI_INTERNALS__) {
      setInitialized(true);
      setLoading(false);
      return;
    }
    accessControlService.getAdminStatus()
      .then((status) => setInitialized(status.initialized))
      .catch((error) => {
        console.warn('Access control is available only in the Tauri application:', error);
        setInitialized(true);
      })
      .finally(() => setLoading(false));
  }, []);

  const login = async () => {
    try {
      const valid = await accessControlService.verifyAdminPassword(password);
      if (!valid) return toast.error('管理员密码不正确');
      setAuthenticated(true);
    } catch (error) {
      if (process.env.NODE_ENV === 'development' && !(window as any).__TAURI_INTERNALS__) {
        setAuthenticated(true);
        toast.info('浏览器开发模式：已跳过 Tauri 本地身份校验');
        return;
      }
      toast.error('无法验证管理员密码', { description: String(error) });
    }
  };

  const initialize = async () => {
    if (newPassword !== confirmPassword) return toast.error('两次输入的新密码不一致');
    if (!/^\d{6}$/.test(newPassword)) return toast.error('管理员密码必须是 6 位数字');
    try {
      await accessControlService.initializeAdminPassword(newPassword);
      setInitialized(true);
      setAuthenticated(true);
      toast.success('首次初始化已完成');
    } catch (error) {
      toast.error('初始化失败', { description: String(error) });
    }
  };

  if (loading) return <div className="hyos-access-loading">正在启动本地安全服务…</div>;
  if (authenticated) return <>{children}</>;

  return <main className="hyos-login-screen"><header className="hyos-login-brand"><span>智能会议助手</span><i>-</i><b>HYOS</b></header><div className="hyos-login-layout"><section className="hyos-device-visual"><div className="hyos-device-shell"><span /><span /><span /><i /></div><h1>会议资料<br />留在本机</h1><p>录音、转写与纪要由设备本地处理。</p></section><section className="hyos-login-card"><div className="hyos-login-mark"><ShieldCheck size={28} /></div><h1>智能会议助手</h1><h2>{initialized ? '管理员登录' : '首次初始化'}</h2><p>{initialized ? '请输入唯一管理员账号与密码。' : '首次使用请设置管理员密码。设备不会预置或显示默认口令。'}</p>{initialized ? <><label>管理员账号<input value="admin" readOnly /></label><label>管理员密码<input type="password" value={password} onChange={(event) => setPassword(event.target.value)} onKeyDown={(event) => event.key === 'Enter' && login()} /></label><button className="hyos-primary hyos-login-submit" onClick={login} disabled={!password}>进入系统 <b>→</b></button></> : <><label>管理员密码<input type="password" inputMode="numeric" maxLength={6} value={newPassword} onChange={(event) => setNewPassword(event.target.value.replace(/\D/g, '').slice(0, 6))} /></label><label>确认管理员密码<input type="password" inputMode="numeric" maxLength={6} value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value.replace(/\D/g, '').slice(0, 6))} /></label><div className="hyos-form-tip">管理员密码必须是 6 位数字。</div><div className="hyos-warning"><b>管理员密码不可找回</b><span>遗忘后需刷写设备，已有数据可能丢失。</span></div><button className="hyos-primary hyos-login-submit" onClick={initialize} disabled={!newPassword || !confirmPassword}>完成初始化并进入系统</button></>}<div className="hyos-login-status"><i />系统服务正常</div></section></div></main>;
}
