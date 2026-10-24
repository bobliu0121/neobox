/**
 * 后台转换任务：把视频转换 / MKA 组合从前端页面状态提升到全局 store。
 *
 * 页面卸载不会中断 Promise，任务结果也不会随组件卸载丢失；重新进入页面
 * 仍能看到运行中 / 已完成状态，完成时由灵动岛统一提示。
 */
import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { showIslandToast } from "@/components/ui/dynamic-island";

export interface VideoConvertResult {
  output_path: string;
  frames: number;
  width: number;
  height: number;
  duration_ms: number;
}

export interface MuxResult {
  output_path: string;
  audio_codec: string;
  subtitle_codec: string | null;
  has_cover: boolean;
  duration_ms: number;
  size: number;
}

/** 音视频倒放 / 变速结果 */
export interface MediaOpResult {
  output_path: string;
  /** 变速完成时的提示（如输出数据量较大），无提示时为 null */
  warning?: string | null;
}

export interface BackgroundJob<T> {
  id: string;
  status: "running" | "done" | "error";
  title: string;
  startedAt: number;
  finishedAt?: number;
  result?: T;
  error?: string;
}

interface VideoConvertParams {
  inputPath: string;
  outputPath: string;
  format: string;
  fps: number;
  width: number;
  startSeconds: number;
  durationSeconds: number;
}

interface MkaMuxParams {
  audioPath: string;
  subtitlePath?: string;
  coverPath?: string;
  outputPath: string;
  title?: string;
}

interface ReverseMediaParams {
  inputPath: string;
  outputPath: string;
}

interface SpeedMediaParams {
  inputPath: string;
  outputPath: string;
  speed: number;
}

interface ConversionState {
  videoJob: BackgroundJob<VideoConvertResult> | null;
  mkaJob: BackgroundJob<MuxResult> | null;
  reverseJob: BackgroundJob<MediaOpResult> | null;
  speedJob: BackgroundJob<MediaOpResult> | null;
  runVideoConvert: (params: VideoConvertParams) => Promise<VideoConvertResult | null>;
  runMkaMux: (params: MkaMuxParams) => Promise<MuxResult | null>;
  runReverseMedia: (params: ReverseMediaParams) => Promise<MediaOpResult | null>;
  runSpeedMedia: (params: SpeedMediaParams) => Promise<MediaOpResult | null>;
  clearVideoJob: () => void;
  clearMkaJob: () => void;
  clearReverseJob: () => void;
  clearSpeedJob: () => void;
}

function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() || path;
}

export const useConversionStore = create<ConversionState>((set) => ({
  videoJob: null,
  mkaJob: null,
  reverseJob: null,
  speedJob: null,

  runVideoConvert: async (params) => {
    const job: BackgroundJob<VideoConvertResult> = {
      id: `video-${Date.now()}`,
      status: "running",
      title: baseName(params.outputPath),
      startedAt: Date.now(),
    };
    set({ videoJob: job });
    showIslandToast({
      id: "background-video",
      title: "视频转换中",
      description: `${baseName(params.outputPath)} · 可离开页面`,
      status: "loading",
      iconKey: "file",
      duration: null,
    });
    try {
      const result = await invoke<VideoConvertResult>("convert_video", {
        inputPath: params.inputPath,
        outputPath: params.outputPath,
        format: params.format,
        fps: params.fps,
        width: params.width,
        startSeconds: params.startSeconds,
        durationSeconds: params.durationSeconds,
      });
      set({
        videoJob: {
          ...job,
          status: "done",
          result,
          finishedAt: Date.now(),
        },
      });
      showIslandToast({
        id: "background-video",
        title: "视频转换完成",
        description: baseName(result.output_path),
        status: "success",
        iconKey: "file",
        duration: 4000,
      });
      return result;
    } catch (error) {
      set({
        videoJob: {
          ...job,
          status: "error",
          error: String(error),
          finishedAt: Date.now(),
        },
      });
      showIslandToast({
        id: "background-video",
        title: "视频转换失败",
        description: String(error),
        status: "error",
        iconKey: "file",
        duration: 7000,
        isClosable: true,
      });
      return null;
    }
  },

  runMkaMux: async (params) => {
    const job: BackgroundJob<MuxResult> = {
      id: `mka-${Date.now()}`,
      status: "running",
      title: baseName(params.outputPath),
      startedAt: Date.now(),
    };
    set({ mkaJob: job });
    showIslandToast({
      id: "background-mka",
      title: "MKA 组合中",
      description: `${baseName(params.outputPath)} · 可离开页面`,
      status: "loading",
      iconKey: "layers",
      duration: null,
    });
    try {
      const result = await invoke<MuxResult>("mux_mka", {
        audioPath: params.audioPath,
        subtitlePath: params.subtitlePath,
        coverPath: params.coverPath,
        outputPath: params.outputPath,
        title: params.title,
      });
      set({
        mkaJob: {
          ...job,
          status: "done",
          result,
          finishedAt: Date.now(),
        },
      });
      showIslandToast({
        id: "background-mka",
        title: "MKA 组合完成",
        description: baseName(result.output_path),
        status: "success",
        iconKey: "layers",
        duration: 4000,
      });
      return result;
    } catch (error) {
      set({
        mkaJob: {
          ...job,
          status: "error",
          error: String(error),
          finishedAt: Date.now(),
        },
      });
      showIslandToast({
        id: "background-mka",
        title: "MKA 组合失败",
        description: String(error),
        status: "error",
        iconKey: "layers",
        duration: 7000,
        isClosable: true,
      });
      return null;
    }
  },

  clearVideoJob: () => set({ videoJob: null }),
  clearMkaJob: () => set({ mkaJob: null }),

  runReverseMedia: async (params) => {
    const job: BackgroundJob<MediaOpResult> = {
      id: `reverse-${Date.now()}`,
      status: "running",
      title: baseName(params.outputPath),
      startedAt: Date.now(),
    };
    set({ reverseJob: job });
    showIslandToast({
      id: "background-reverse",
      title: "音视频倒放中",
      description: `${baseName(params.outputPath)} · 可离开页面`,
      status: "loading",
      iconKey: "file",
      duration: null,
    });
    try {
      await invoke("reverse_media", {
        inputPath: params.inputPath,
        outputPath: params.outputPath,
      });
      const result: MediaOpResult = { output_path: params.outputPath, warning: null };
      set({
        reverseJob: {
          ...job,
          status: "done",
          result,
          finishedAt: Date.now(),
        },
      });
      showIslandToast({
        id: "background-reverse",
        title: "音视频倒放完成",
        description: baseName(result.output_path),
        status: "success",
        iconKey: "file",
        duration: 4000,
      });
      return result;
    } catch (error) {
      set({
        reverseJob: {
          ...job,
          status: "error",
          error: String(error),
          finishedAt: Date.now(),
        },
      });
      showIslandToast({
        id: "background-reverse",
        title: "音视频倒放失败",
        description: String(error),
        status: "error",
        iconKey: "file",
        duration: 7000,
        isClosable: true,
      });
      return null;
    }
  },

  runSpeedMedia: async (params) => {
    const job: BackgroundJob<MediaOpResult> = {
      id: `speed-${Date.now()}`,
      status: "running",
      title: baseName(params.outputPath),
      startedAt: Date.now(),
    };
    set({ speedJob: job });
    showIslandToast({
      id: "background-speed",
      title: "音视频加速/减速中",
      description: `${baseName(params.outputPath)} · 可离开页面`,
      status: "loading",
      iconKey: "file",
      duration: null,
    });
    try {
      const warning = await invoke<string | null>("change_media_speed", {
        inputPath: params.inputPath,
        outputPath: params.outputPath,
        speed: params.speed,
      });
      const result: MediaOpResult = { output_path: params.outputPath, warning };
      set({
        speedJob: {
          ...job,
          status: "done",
          result,
          finishedAt: Date.now(),
        },
      });
      showIslandToast({
        id: "background-speed",
        title: "音视频加速/减速完成",
        description: baseName(result.output_path),
        status: "success",
        iconKey: "file",
        duration: 4000,
      });
      return result;
    } catch (error) {
      set({
        speedJob: {
          ...job,
          status: "error",
          error: String(error),
          finishedAt: Date.now(),
        },
      });
      showIslandToast({
        id: "background-speed",
        title: "音视频加速/减速失败",
        description: String(error),
        status: "error",
        iconKey: "file",
        duration: 7000,
        isClosable: true,
      });
      return null;
    }
  },

  clearReverseJob: () => set({ reverseJob: null }),
  clearSpeedJob: () => set({ speedJob: null }),
}));
