'use client';

import { useCallback, useRef, useReducer, startTransition, useEffect, useState, memo, useMemo } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useAutoScroll } from "@/hooks/useAutoScroll";
import { useTranscriptStreaming } from "@/hooks/useTranscriptStreaming";
import { ConfidenceIndicator } from "./ConfidenceIndicator";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";
import { RecordingStatusBar } from "./RecordingStatusBar";
import { motion } from "framer-motion";
import { TranscriptSegmentData } from "@/types";
import { uiText } from "@/lib/uiText";
import { Headphones, Pencil } from "lucide-react";

export interface VirtualizedTranscriptViewProps {
    /** Transcript segments to display */
    segments: TranscriptSegmentData[];
    /** Whether recording is in progress */
    isRecording?: boolean;
    /** Whether recording is paused */
    isPaused?: boolean;
    /** Whether processing/finalizing transcription */
    isProcessing?: boolean;
    /** Whether stopping */
    isStopping?: boolean;
    /** Enable streaming effect for latest segment */
    enableStreaming?: boolean;
    /** Show confidence indicators */
    showConfidence?: boolean;
    /** Completely disable auto-scroll behavior (for meeting details page) */
    disableAutoScroll?: boolean;

    // Pagination props (infinite scroll)
    hasMore?: boolean;
    isLoadingMore?: boolean;
    totalCount?: number;
    loadedCount?: number;
    onLoadMore?: () => void;
    onPlaySegment?: (timestamp: number, endTime?: number) => void;
    onEditSegment?: (segment: TranscriptSegmentData) => void;
}

// Threshold for enabling virtualization (below this, use simple rendering)
const VIRTUALIZATION_THRESHOLD = 10;

function splitSpeakerPrefix(text: string, explicitLabel?: string): { speakerLabel?: string; text: string } {
    const match = text.match(/^((?:候选发言人|发言人待识别|发言人(?:\s*[^：:\s]{1,20})?))[：:]\s*([\s\S]*)$/);
    if (explicitLabel) return { speakerLabel: explicitLabel, text: match?.[2] ?? text };
    return match ? { speakerLabel: match[1], text: match[2] } : { text };
}


// Adapted from Brevia backend/worker_session.py (_coalesce_subtitles, _join_text).
// ISC License; Copyright (c) 2026 Brevia contributors. See THIRD_PARTY_BREVIA.txt.
// Display-only grouping keeps original records for seeking/editing and shows the tail immediately.
function paragraphSeparator(left: string, right: string): string {
    if (!left || !right || /\s$/.test(left) || /^\s/.test(right)) return '';
    return /[\x00-\x7F]$/.test(left) && /^[A-Za-z0-9]/.test(right) ? ' ' : '';
}

function isSilenceMarker(text: string): boolean {
    return /^\s*(?:[<\[({]?\s*(?:sil|silence|no_speech)\s*[>\])}]?[\s.,，。!?！？;；]*)+$/i.test(text);
}

function groupTranscriptSegments(source: TranscriptSegmentData[]) {
    const groups: (TranscriptSegmentData & { parts: TranscriptSegmentData[] })[] = [];
    for (const item of source) {
        const parsed = splitSpeakerPrefix(item.text, item.speakerLabel);
        if (!parsed.text.trim() || isSilenceMarker(parsed.text)) continue;
        const part = { ...item, ...parsed };
        const previous = groups[groups.length - 1];
        const tail = previous?.parts[previous.parts.length - 1];
        const separator = previous ? paragraphSeparator(previous.text, part.text) : '';
        const held = previous?.text.length ?? 0;
        const complete = previous ? /[。！？!?][”’」』）)]*$/.test(previous.text.trim()) : false;
        const sameSpeaker = parsed.speakerLabel && !/待识别|候选/.test(parsed.speakerLabel) && previous?.speakerLabel === parsed.speakerLabel;
        // Same-speaker runs form reading paragraphs; silence belongs to ASR endpointing.
        // Start a new paragraph at a completed sentence after reaching the reading target.
        const ordered = !tail || part.timestamp >= tail.timestamp;
        if (previous && sameSpeaker && ordered && !(held >= 200 && complete)) {
            previous.parts.push(part);
            previous.text += separator + part.text;
            previous.endTime = part.endTime;
        } else {
            groups.push({ ...part, parts: [part] });
        }
    }
    return groups;
}

// Helper function to format seconds as recording-relative time [MM:SS]
function formatRecordingTime(seconds: number | undefined): string {
    if (seconds === undefined) return '[--:--]';

    const totalSeconds = Math.floor(seconds);
    const minutes = Math.floor(totalSeconds / 60);
    const secs = totalSeconds % 60;

    return `[${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
}

// Helper function to remove filler words and repetitions
function cleanStopWords(text: string): string {
    const stopWords = ['uh', 'um', 'er', 'ah', 'hmm', 'hm', 'eh', 'oh'];

    let cleanedText = text;
    stopWords.forEach(word => {
        const pattern = new RegExp(`\\b${word}\\b[,\\s]*`, 'gi');
        cleanedText = cleanedText.replace(pattern, ' ');
    });

    return cleanedText.replace(/\s+/g, ' ').trim();
}

// Memoized transcript segment component
const TranscriptSegment = memo(function TranscriptSegment({
    id,
    timestamp,
    endTime,
    text,
    confidence,
    speakerLabel,
    isStreaming,
    showConfidence,
    onPlaySegment,
    onEditSegment,
    parts,
}: {
    parts?: TranscriptSegmentData[];
    id: string;
    timestamp: number;
    endTime?: number;
    text: string;
    confidence?: number;
    speakerLabel?: string;
    isStreaming: boolean;
    showConfidence: boolean;
    onPlaySegment?: (timestamp: number, endTime?: number) => void;
    onEditSegment?: (segment: TranscriptSegmentData) => void;
}) {
    const parsed = splitSpeakerPrefix(text, speakerLabel);
    const displayText = cleanStopWords(parsed.text) || (parsed.text.trim() === '' ? uiText.silence : parsed.text);

    return (
        <div id={`segment-${id}`} className="mb-5 rounded-lg border border-slate-100 bg-slate-50/50 px-4 py-3">
            <div className="flex items-start gap-2">
                <Tooltip>
                    <TooltipTrigger>
                        <span className="text-xs text-gray-400 mt-1 flex-shrink-0 min-w-[50px]">
                            {formatRecordingTime(timestamp)}
                        </span>
                    </TooltipTrigger>
                    <TooltipContent>
                        {confidence !== undefined && showConfidence && (
                            <ConfidenceIndicator confidence={confidence} showIndicator={showConfidence} />
                        )}
                    </TooltipContent>
                </Tooltip>
                <div className="min-w-0 flex-1 [overflow-wrap:anywhere]">
                    {parsed.speakerLabel && <div className="mb-1 text-xs font-semibold text-blue-700">{parsed.speakerLabel}</div>}
                    {isStreaming ? (
                        <div className="bg-gray-100 border border-gray-200 rounded-lg px-3 py-2">
                            <p className="text-base text-gray-800 leading-8">{parts && parts.length > 1 ? parts.map((part, index) => (
                            <span key={part.id} className="group/phrase">
                                {index > 0 ? paragraphSeparator(parts[index - 1].text, part.text) : ''}{part.text}
                                {onEditSegment && <button className="ml-1 inline-flex text-gray-300 hover:text-blue-600" title={`校对 ${formatRecordingTime(part.timestamp)} 这一句`} onClick={() => onEditSegment(part)}><Pencil size={12} /></button>}
                            </span>
                        )) : displayText}</p>
                        </div>
                    ) : (
                        <p className="text-base text-gray-800 leading-8">{parts && parts.length > 1 ? parts.map((part, index) => (
                            <span key={part.id} className="group/phrase">
                                {index > 0 ? paragraphSeparator(parts[index - 1].text, part.text) : ''}{part.text}
                                {onEditSegment && <button className="ml-1 inline-flex text-gray-300 hover:text-blue-600" title={`校对 ${formatRecordingTime(part.timestamp)} 这一句`} onClick={() => onEditSegment(part)}><Pencil size={12} /></button>}
                            </span>
                        )) : displayText}</p>
                    )}
                </div>
                {onPlaySegment && (
                    <button
                        type="button"
                        className="mt-0.5 flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-md text-gray-400 hover:bg-blue-50 hover:text-blue-600"
                        onClick={() => onPlaySegment(timestamp, endTime)}
                        title="播放这一段录音"
                        aria-label="播放这一段录音"
                    >
                        <Headphones size={15} />
                    </button>
                )}
                {onEditSegment && !isStreaming && (!parts || parts.length <= 1) && (
                    <button
                        type="button"
                        className="mt-0.5 flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-md text-gray-400 hover:bg-blue-50 hover:text-blue-600"
                        onClick={() => onEditSegment({ id, timestamp, endTime, text: parsed.text, confidence, speakerLabel: parsed.speakerLabel })}
                        title="校对这一段转写"
                        aria-label="校对这一段转写"
                    >
                        <Pencil size={14} />
                    </button>
                )}
            </div>
        </div>
    );
});

export const VirtualizedTranscriptView: React.FC<VirtualizedTranscriptViewProps> = ({
    segments: sourceSegments,
    isRecording = false,
    isPaused = false,
    isProcessing = false,
    isStopping = false,
    enableStreaming = false,
    showConfidence = true,
    disableAutoScroll = false,
    hasMore = false,
    isLoadingMore = false,
    totalCount = 0,
    loadedCount = 0,
    onLoadMore,
    onPlaySegment,
    onEditSegment,
}) => {
    const [showLiveCaptions, setShowLiveCaptions] = useState(true);
    const captionSegments = useMemo(() => sourceSegments.map(item => ({
        ...item, ...splitSpeakerPrefix(item.text, item.speakerLabel),
    })).filter(item => item.text.trim() && !isSilenceMarker(item.text)).slice(-2), [sourceSegments]);
    const segments = useMemo(() => groupTranscriptSegments(sourceSegments), [sourceSegments]);
    // Create scroll ref first - shared between virtualizer and auto-scroll hook
    const scrollRef = useRef<HTMLDivElement>(null);
    // Ref for infinite scroll trigger element
    const loadMoreTriggerRef = useRef<HTMLDivElement>(null);

    // Force re-render without flushSync (avoids React warning)
    const [, rerender] = useReducer((x: number) => x + 1, 0);

    // Setup virtualizer for efficient rendering of large lists
    const virtualizer = useVirtualizer({
        count: segments.length,
        getItemKey: (index) => segments[index].id,
        getScrollElement: () => scrollRef.current,
        estimateSize: () => 96,
        overscan: 10, // Render extra items above/below viewport
        onChange: () => {
            startTransition(() => {
                rerender();
            });
        },
    });

    // Custom hook for auto-scrolling (supports both virtualized and non-virtualized)
    useAutoScroll({
        scrollRef,
        segments,
        isRecording: isRecording || isProcessing || isStopping,
        isPaused,
        virtualizer,
        virtualizationThreshold: VIRTUALIZATION_THRESHOLD,
        disableAutoScroll,
    });

    // Streaming text effect hook (typewriter animation for new transcripts)
    const { streamingSegmentId, getDisplayText } = useTranscriptStreaming(
        segments,
        isRecording,
        enableStreaming
    );

    // Infinite scroll: IntersectionObserver to trigger loading more
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording || segments.length === 0) {
            return;
        }

        const triggerElement = loadMoreTriggerRef.current;
        if (!triggerElement) return;

        const observer = new IntersectionObserver(
            (entries) => {
                if (entries[0].isIntersecting && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
            },
            {
                root: null,
                rootMargin: '100px',
                threshold: 0,
            }
        );

        observer.observe(triggerElement);

        return () => observer.disconnect();
    }, [hasMore, isLoadingMore, onLoadMore, isRecording, segments.length]);

    // Scroll-based fallback for fast scrolling
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording) return;

        const scrollElement = scrollRef.current;
        if (!scrollElement) return;

        let ticking = false;

        const handleScroll = () => {
            if (ticking || isLoadingMore || !hasMore) return;

            ticking = true;
            requestAnimationFrame(() => {
                const { scrollTop, scrollHeight, clientHeight } = scrollElement;
                const scrollBottom = scrollHeight - scrollTop - clientHeight;

                // Trigger load when within 200px of bottom
                if (scrollBottom < 200 && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
                ticking = false;
            });
        };

        scrollElement.addEventListener('scroll', handleScroll, { passive: true });
        return () => scrollElement.removeEventListener('scroll', handleScroll);
    }, [onLoadMore, hasMore, isLoadingMore, isRecording]);

    // Use simple rendering for small lists, virtualization for large lists
    const useVirtualization = segments.length >= VIRTUALIZATION_THRESHOLD;

    return (
        <div className="flex flex-col h-full min-h-0">
            {isRecording && (
                <section className="shrink-0 mx-4 mb-3 rounded-xl border border-blue-100 bg-blue-50/70 p-4" aria-label="实时字幕">
                    <div className="flex items-center justify-between gap-3 mb-2">
                        <span className="text-sm font-semibold text-blue-800">实时字幕 · {isPaused ? '已暂停' : '正在聆听'}</span>
                        <button type="button" className="text-xs text-blue-700 hover:underline" onClick={() => setShowLiveCaptions(value => !value)} aria-expanded={showLiveCaptions}>
                            {showLiveCaptions ? '收起字幕' : '展开字幕'}
                        </button>
                    </div>
                    {showLiveCaptions && <div role="log" aria-live="polite" aria-atomic="true" className="max-h-44 overflow-y-auto space-y-2 text-xl leading-relaxed text-slate-900">
                        {captionSegments.length ? captionSegments.map((segment, index) => <p key={segment.id} className={index < captionSegments.length - 1 ? 'text-slate-500 text-base' : ''}>
                            {segment.speakerLabel && <span className="mr-2 text-xs font-medium text-blue-700">{segment.speakerLabel}</span>}
                            {segment.text}
                        </p>) : <p className="text-base text-slate-500">{isPaused ? '录音已暂停' : '等待语音，识别内容将在这里实时显示…'}</p>}
                    </div>}
                </section>
            )}
            {isRecording && (
                <div className="shrink-0 px-4 pt-2 bg-white">
                    <RecordingStatusBar isPaused={isPaused} />
                </div>
            )}

            <div ref={scrollRef} className="flex-1 min-h-0 overflow-y-auto px-4 py-2">
            {segments.length === 0 ? (
                // Empty state
                <motion.div
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    className="text-center text-gray-500 mt-8"
                >
                    {isRecording ? (
                        <>
                            <div className="flex items-center justify-center mb-3">
                                <div className={`w-3 h-3 rounded-full ${isPaused ? 'bg-orange-500' : 'bg-blue-500 animate-pulse'}`}></div>
                            </div>
                            <p className="text-sm text-gray-600">
                                {isPaused ? uiText.recordingPaused : uiText.listening}
                            </p>
                            <p className="text-xs mt-1 text-gray-400">
                                {isPaused ? uiText.clickResume : uiText.speakToSee}
                            </p>
                        </>
                    ) : (
                        <>
                            <p className="text-lg font-semibold">{uiText.welcomeTitle}</p>
                            <p className="text-xs mt-1">{uiText.welcomeHint}</p>
                        </>
                    )}
                </motion.div>
            ) : useVirtualization ? (
                // Virtualized rendering for large lists
                <>
                    <div
                        style={{
                            height: virtualizer.getTotalSize(),
                            width: "100%",
                            position: "relative",
                        }}
                    >
                        {virtualizer.getVirtualItems().map((virtualRow) => {
                            const segment = segments[virtualRow.index];
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <div
                                    key={segment.id}
                                    data-index={virtualRow.index}
                                    ref={virtualizer.measureElement}
                                    className="flow-root"
                                    style={{
                                        position: "absolute",
                                        top: 0,
                                        left: 0,
                                        width: "100%",
                                        transform: `translateY(${virtualRow.start}px)`,
                                    }}
                                >
                                    <TranscriptSegment
                                        parts={segment.parts}
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        endTime={segment.endTime}
                                        text={getDisplayText(segment)}
                                        confidence={segment.confidence}
                                        speakerLabel={segment.speakerLabel}
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        onPlaySegment={onPlaySegment}
                                        onEditSegment={onEditSegment}
                                    />
                                </div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger and loading indicator */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-gray-500">
                                    <div className="w-4 h-4 border-2 border-gray-300 border-t-gray-600 rounded-full animate-spin" />
                                    <span className="text-sm">{uiText.loadingMore}</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-gray-400">
                                    {uiText.showingSegments(loadedCount, totalCount)}
                                </span>
                            ) : null}
                        </div>
                    )}

                    {/* Listening indicator when recording */}
                    {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                        <motion.div
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="flex items-center gap-2 mt-4 text-gray-500"
                        >
                            <div className="w-2 h-2 bg-blue-500 rounded-full animate-pulse"></div>
                            <span className="text-sm">{uiText.listening}</span>
                        </motion.div>
                    )}
                </>
            ) : (
                // Simple rendering for small lists (better animations)
                <>
                    <div className="space-y-1">
                        {segments.map((segment) => {
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <motion.div
                                    key={segment.id}
                                    initial={{ opacity: 0, y: 5 }}
                                    animate={{ opacity: 1, y: 0 }}
                                    transition={{ duration: 0.15 }}
                                >
                                    <TranscriptSegment
                                        parts={segment.parts}
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        endTime={segment.endTime}
                                        text={getDisplayText(segment)}
                                        confidence={segment.confidence}
                                        speakerLabel={segment.speakerLabel}
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        onPlaySegment={onPlaySegment}
                                        onEditSegment={onEditSegment}
                                    />
                                </motion.div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger (for small lists that grow) */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-gray-500">
                                    <div className="w-4 h-4 border-2 border-gray-300 border-t-gray-600 rounded-full animate-spin" />
                                    <span className="text-sm">{uiText.loadingMore}</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-gray-400">
                                    {uiText.showingSegments(loadedCount, totalCount)}
                                </span>
                            ) : null}
                        </div>
                    )}

                    {/* Listening indicator when recording */}
                    {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                        <motion.div
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="flex items-center gap-2 mt-4 text-gray-500"
                        >
                            <div className="w-2 h-2 bg-blue-500 rounded-full animate-pulse"></div>
                            <span className="text-sm">{uiText.listening}</span>
                        </motion.div>
                    )}
                </>
            )}
            </div>
        </div>
    );
};
