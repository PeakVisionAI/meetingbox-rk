import React, { useState, useEffect } from "react";
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import AnalyticsConsentSwitch from "./AnalyticsConsentSwitch";
import { UpdateDialog } from "./UpdateDialog";
import { updateService, UpdateInfo } from '@/services/updateService';
import { Button } from './ui/button';
import { Loader2, CheckCircle2 } from 'lucide-react';
import { toast } from 'sonner';
import { uiText } from '@/lib/uiText';


export function About() {
    const [currentVersion, setCurrentVersion] = useState<string>('0.4.0');
    const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
    const [isChecking, setIsChecking] = useState(false);
    const [showUpdateDialog, setShowUpdateDialog] = useState(false);

    useEffect(() => {
        // Get current version on mount
        getVersion().then(setCurrentVersion).catch(console.error);
    }, []);

    const handleContactClick = async () => {
        try {
            await invoke('open_external_url', { url: 'https://meetily.zackriya.com/#about' });
        } catch (error) {
            console.error('Failed to open link:', error);
        }
    };

    const handleCheckForUpdates = async () => {
        setIsChecking(true);
        try {
            const info = await updateService.checkForUpdates(true);
            setUpdateInfo(info);
            if (info.available) {
                setShowUpdateDialog(true);
            } else {
                toast.success(uiText.latestVersion);
            }
        } catch (error: any) {
            console.error('Failed to check for updates:', error);
            toast.error(uiText.checkForUpdates + '：' + (error.message || '未知错误'));
        } finally {
            setIsChecking(false);
        }
    };

    return (
        <div className="p-4 space-y-4 h-[80vh] overflow-y-auto">
            {/* Compact Header */}
            <div className="text-center">
                <h1 className="text-xl font-bold text-gray-900">{uiText.appName}</h1>
                <span className="text-sm text-gray-500">v{currentVersion}</span>
                <p className="text-medium text-gray-600 mt-1">
                    {uiText.aboutTagline}
                </p>
                <div className="mt-3">
                    <Button
                        onClick={handleCheckForUpdates}
                        disabled={isChecking}
                        variant="outline"
                        size="sm"
                        className="text-xs"
                    >
                        {isChecking ? (
                            <>
                                <Loader2 className="h-3 w-3 mr-2 animate-spin" />
                                {uiText.checkingUpdates}
                            </>
                        ) : (
                            <>
                                <CheckCircle2 className="h-3 w-3 mr-2" />
                                {uiText.checkForUpdates}
                            </>
                        )}
                    </Button>
                    {updateInfo?.available && (
                        <div className="mt-2 text-xs text-blue-600">
                            {uiText.updateAvailable(updateInfo.version ?? updateInfo.currentVersion)}
                        </div>
                    )}
                </div>
            </div>

            {/* Features Grid - Compact */}
            <div className="space-y-3">
                <h2 className="text-base font-semibold text-gray-800">{uiText.aboutDifference}</h2>
                <div className="grid grid-cols-2 gap-2">
                    <div className="bg-gray-50 rounded p-3 hover:bg-gray-100 transition-colors">
                        <h3 className="font-bold text-sm text-gray-900 mb-1">{uiText.privacyFirst}</h3>
                        <p className="text-xs text-gray-600 leading-relaxed">{uiText.privacyFirstHint}</p>
                    </div>
                    <div className="bg-gray-50 rounded p-3 hover:bg-gray-100 transition-colors">
                        <h3 className="font-bold text-sm text-gray-900 mb-1">{uiText.useAnyModel}</h3>
                        <p className="text-xs text-gray-600 leading-relaxed">{uiText.useAnyModelHint}</p>
                    </div>
                    <div className="bg-gray-50 rounded p-3 hover:bg-gray-100 transition-colors">
                        <h3 className="font-bold text-sm text-gray-900 mb-1">{uiText.costSmart}</h3>
                        <p className="text-xs text-gray-600 leading-relaxed">{uiText.costSmartHint}</p>
                    </div>
                    <div className="bg-gray-50 rounded p-3 hover:bg-gray-100 transition-colors">
                        <h3 className="font-bold text-sm text-gray-900 mb-1">{uiText.worksEverywhere}</h3>
                        <p className="text-xs text-gray-600 leading-relaxed">{uiText.worksEverywhereHint}</p>
                    </div>
                </div>
            </div>

            {/* Coming Soon - Compact */}
            <div className="bg-blue-50 rounded p-3">
                <p className="text-s text-blue-800">
                    <span className="font-bold">即将推出：</span> 本机 AI 助手库，用于跟进待办、追踪行动项等。
                </p>
            </div>

            {/* CTA Section - Compact */}
            <div className="text-center space-y-2">
                <h3 className="text-medium font-semibold text-gray-800">需要定制方案？</h3>
                <p className="text-s text-gray-600">
                    如果要为业务打造隐私优先的定制 AI 助手或完整产品，可以联系我们。
                </p>
                <button
                    onClick={handleContactClick}
                    className="inline-flex items-center px-4 py-2 bg-blue-600 hover:bg-blue-700 text-white text-sm font-medium rounded transition-colors duration-200 shadow-sm hover:shadow-md"
                >
                    联系团队
                </button>
            </div>

            {/* Footer - Compact */}
            <div className="pt-2 border-t border-gray-200 text-center">
                <p className="text-xs text-gray-400">
                    由 Zackriya Solutions 构建
                </p>
            </div>
            <AnalyticsConsentSwitch />

            {/* Update Dialog */}
            <UpdateDialog
                open={showUpdateDialog}
                onOpenChange={setShowUpdateDialog}
                updateInfo={updateInfo}
            />
        </div>

    )
}
