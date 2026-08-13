'use client';

import React from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePathname, useRouter } from 'next/navigation';
import { Settings, FileText, Play } from 'lucide-react';

interface MainContentProps {
  children: React.ReactNode;
}

const MainContent: React.FC<MainContentProps> = ({ children }) => {
  const router = useRouter();
  const pathname = usePathname();
  const { isMeetingActive } = useSidebar();
  const isHome = pathname === '/';
  const isFiles = pathname === '/files';
  const isSettings = pathname === '/settings';

  return (
    <main className="hyos-main">
      <header className="hyos-topbar">
        <button className="hyos-brand" onClick={() => router.push('/')} aria-label="返回工作台">
          <span>智能会议助手</span><i>-</i><b>HYOS</b>
        </button>
        <nav className="hyos-nav" aria-label="主功能导航">
          <button className={isHome ? 'active' : ''} onClick={() => router.push('/')}><Play size={15} />快速会议</button>
          <button className={isFiles ? 'active' : ''} onClick={() => router.push('/files')}><FileText size={15} />文件管理</button>
          <button className={isSettings ? 'active' : ''} onClick={() => router.push('/settings')}><Settings size={15} />设置</button>
        </nav>
        <div className="hyos-user"><span className={isMeetingActive ? 'recording-dot' : 'status-dot'} />{isMeetingActive ? '会议进行中' : '已登录'}</div>
      </header>
      <div className="hyos-content">{children}</div>
    </main>
  );
};

export default MainContent;
