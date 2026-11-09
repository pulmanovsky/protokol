'use client';

import React from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';

interface MainContentProps {
  children: React.ReactNode;
}

const MainContent: React.FC<MainContentProps> = ({ children }) => {
  const { isCollapsed } = useSidebar();

  return (
    <main
      className={`content-area flex-1 min-w-0 h-screen overflow-hidden transition-[margin] duration-200 ${
        isCollapsed ? 'ml-[90px]' : 'ml-[260px]'
      }`}
    >
      {children}
    </main>
  );
};

export default MainContent;
