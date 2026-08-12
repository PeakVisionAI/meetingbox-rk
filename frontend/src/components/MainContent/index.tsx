'use client';

import React from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import HyosHeader from '@/components/HyosHeader';

interface MainContentProps {
  children: React.ReactNode;
}

const MainContent: React.FC<MainContentProps> = ({ children }) => {
  const { isCollapsed } = useSidebar();

  return (
    <main
      className={`hyos-main transition-all duration-300 ${isCollapsed ? 'ml-16' : 'ml-64'}`}
    >
      <HyosHeader />
      <div className="hyos-page-content">
        {children}
      </div>
    </main>
  );
};

export default MainContent;
