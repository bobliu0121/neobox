"use client";

import {
  createContext,
  useContext,
  useState,
  useEffect,
  useCallback,
  useMemo,
  useRef,
  type ReactNode,
} from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useDynamicIsland } from "@/components/ui/dynamic-island";
import { useTranslation } from "react-i18next";
import { store } from "@/lib/store";
import { useAppStartup } from "@/contexts/app-startup-context";
import { fetchLatestRelease, compareVersions, type ReleaseInfo } from "@/lib/update-checker";
import { IS_STORE_BUILD } from "@/lib/build-flags";

const CURRENT_VERSION = "v1.3.0";
const AUTO_UPDATE_KEY = "neobox_auto_update";
/** 灵动岛更新下载岛的固定 id */
const UPDATE_ISLAND_ID = "update-download";

interface UpdateContextValue {
  latestRelease: ReleaseInfo | null;
  hasUpdate: boolean;
  isChecking: boolean;
  isDownloading: boolean;
  downloadProgress: number;
  isDownloadComplete: boolean;
  downloadedFilePath: string;
  isModalOpen: boolean;
  autoUpdateEnabled: boolean;
  setAutoUpdateEnabled: (value: boolean) => Promise<void>;
  openModal: () => void;
  closeModal: () => void;
  toggleModal: () => void;
  handleCheckUpdate: () => Promise<void>;
  manualDownload: () => Promise<void>;
  handleInstall: () => Promise<void>;
  handleSkip: () => Promise<void>;
}

const UpdateContext = createContext<UpdateContextValue | null>(null);

export function useUpdate() {
  const ctx = useContext(UpdateContext);
  if (!ctx) {
    throw new Error("useUpdate must be used within UpdateProvider");
  }
  return ctx;
}

export function UpdateProvider({ children }: { children: ReactNode }) {
  const { isStartupComplete } = useAppStartup();
  const navigate = useNavigate();
  const toast = useDynamicIsland("download");
  const { t } = useTranslation();

  const [autoUpdateEnabled, setAutoUpdateEnabledState] = useState(true);
  const [latestRelease, setLatestRelease] = useState<ReleaseInfo | null>(null);
  const [hasUpdate, setHasUpdate] = useState(false);
  const [isChecking, setIsChecking] = useState(false);
  const [isDownloading, setIsDownloading] = useState(false);
  const [autoUpdateLoaded, setAutoUpdateLoaded] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState(0); // 目标进度
  const [displayProgress, setDisplayProgress] = useState(0); // 平滑显示的进度
  const [isDownloadComplete, setIsDownloadComplete] = useState(false);
  const [downloadedFilePath, setDownloadedFilePath] = useState<string>("");
  const [isModalOpen, setIsModalOpen] = useState(false);

  const downloadStartedRef = useRef(false);
  const autoUpdateLoadedRef = useRef(false);
  const autoUpdateEnabledRef = useRef(true);
  const downloadCancelledRef = useRef(false);
  const isDownloadingRef = useRef(false);
  const targetProgressRef = useRef(0);
  const displayProgressRef = useRef(0);
  // 下载完成后的安装包路径 ref：供灵动岛内 onClick 等闭包读取最新值，避免过期捕获
  const downloadedFilePathRef = useRef("");
  // 完成态标记：进度监听器据此阻止回写，避免延迟事件覆盖「点击重启」导致进度条回退/闪烁
  const isDownloadCompleteRef = useRef(false);
  // 岛内进度只涨不跌的保险
  const lastIslandProgressRef = useRef(0);
  // 内置下载器任务 id
  const accelTaskIdRef = useRef<string>("");

  // 平滑动画：显示值每 50ms 向目标值逼近，且强制单调递增、永不倒退
  useEffect(() => {
    const id = setInterval(() => {
      const target = targetProgressRef.current;
      const current = displayProgressRef.current;
      // 目标值 ≤ 当前显示值时保持不动（进度只涨不跌）；否则缓动逼近。
      // ⚠️ 必须「吸附」：缓动 forever 只会无限逼近而永远到不了目标，
      // 实测会在 99.6% 附近反复逼近却到不了 100%，用户看到的就是
      // 「进度显示不正确 / 卡在 99% 不动」。差值 < 0.5 时直接跳到目标值。
      const diff = target - current;
      const next = diff <= 0 ? current : (diff < 0.5 ? target : current + diff * 0.4);
      if (next !== current) {
        displayProgressRef.current = next;
        setDisplayProgress(next);
      }
    }, 50);
    return () => clearInterval(id);
  }, []);

  // 读取静默更新设置（默认开启，强制布尔化防止字符串/数字等 truthy 假值误判）
  useEffect(() => {
    (async () => {
      try {
        const raw = await store.get<unknown>(AUTO_UPDATE_KEY);
        let v: boolean;
        if (raw === null || raw === undefined) {
          const ls = localStorage.getItem(AUTO_UPDATE_KEY);
          v = ls === null ? true : ls === "true";
        } else {
          v = raw === true || raw === "true" || raw === 1;
        }
        setAutoUpdateEnabledState(v);
        autoUpdateEnabledRef.current = v;
      } catch (error) {
        console.error("Failed to load auto update setting:", error);
      } finally {
        autoUpdateLoadedRef.current = true;
        setAutoUpdateLoaded(true);
      }
    })();
  }, []);

  // 监听静默更新设置变更广播（设置页等同步）
  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent<boolean>).detail;
      if (typeof detail === "boolean") {
        setAutoUpdateEnabledState(detail);
        autoUpdateEnabledRef.current = detail;
      }
    };
    window.addEventListener("auto-update-setting-changed", handler as EventListener);
    return () => {
      window.removeEventListener("auto-update-setting-changed", handler as EventListener);
    };
  }, []);

  // 保持 ref 与 state 同步，供回调读取最新值
  useEffect(() => {
    autoUpdateEnabledRef.current = autoUpdateEnabled;
  }, [autoUpdateEnabled]);
  useEffect(() => {
    isDownloadingRef.current = isDownloading;
  }, [isDownloading]);
  useEffect(() => {
    downloadedFilePathRef.current = downloadedFilePath;
  }, [downloadedFilePath]);
  useEffect(() => {
    isDownloadCompleteRef.current = isDownloadComplete;
  }, [isDownloadComplete]);

  const setAutoUpdateEnabled = useCallback(async (value: boolean) => {
    setAutoUpdateEnabledState(value);
    autoUpdateEnabledRef.current = value;
    localStorage.setItem(AUTO_UPDATE_KEY, String(value));
    window.dispatchEvent(new CustomEvent("auto-update-setting-changed", { detail: value }));
    if (!value) {
      // 关闭静默更新：立即置取消标志（先于任何 await），确保下载 catch 分支静默处理
      downloadCancelledRef.current = true;
    }
    try {
      await store.set(AUTO_UPDATE_KEY, value);
      await store.save();
    } catch (error) {
      console.error("Failed to save auto update setting:", error);
    }
    if (!value) {
      // 关闭静默更新：立即取消进行中的下载、清除待安装标记，关闭软件不再自动更新
      try {
        if (accelTaskIdRef.current) {
          await invoke("accel_cancel", { id: accelTaskIdRef.current });
        } else {
          await invoke("cancel_download");
        }
      } catch (error) {
        console.error("Failed to cancel download:", error);
      }
      try {
        await invoke("clear_pending_install");
      } catch (error) {
        console.error("Failed to clear pending install:", error);
      }
      // 仅当下载已不在进行时才重置互斥锁；进行中的下载由其后端中止后的分支负责重置，
      // 避免用户重新开启后触发并发下载
      if (!isDownloadingRef.current) {
        downloadStartedRef.current = false;
      }
      setIsDownloading(false);
      setIsDownloadComplete(false);
      setDownloadedFilePath("");
      setDownloadProgress(0);
      setDisplayProgress(0);
      targetProgressRef.current = 0;
      displayProgressRef.current = 0;
      // 关闭更新弹窗与"有新版本"标记，彻底隐藏更新入口
      setIsModalOpen(false);
      setHasUpdate(false);
      // 关闭灵动岛更新下载岛
      toast.closePersistent(UPDATE_ISLAND_ID);
    }
  }, [toast]);

  // 下载进度事件：后端已按 200ms 节流，此处直接更新目标值，并静默同步灵动岛进度
  useEffect(() => {
    const unlisten = listen<{ progress: number; total: number }>("download-progress", (event) => {
      targetProgressRef.current = event.payload.progress;
      setDownloadProgress(event.payload.progress);
      // 完成态不再回写进度，避免延迟事件覆盖「点击重启」导致进度条回退/闪烁
      if (isDownloadCompleteRef.current) return;
      // 岛内进度只涨不跌（保险，杜绝任何回退）
      const next = Math.max(lastIslandProgressRef.current, event.payload.progress);
      lastIslandProgressRef.current = next;
      // 静默更新（animate=false），避免每 200ms 触发缩→扩动画
      toast.updatePersistent(UPDATE_ISLAND_ID, { progress: next }, false);
    });
    // 监听内置下载器进度事件
    const unlistenAccel = listen<{
      id: string;
      downloadedBytes: number;
      totalBytes: number;
      status: number;
    }>("accel-progress", (event) => {
      if (event.payload.id !== accelTaskIdRef.current) return;
      if (isDownloadCompleteRef.current) return;
      const pct = event.payload.totalBytes > 0
        ? (event.payload.downloadedBytes / event.payload.totalBytes) * 100
        : 0;
      targetProgressRef.current = pct;
      setDownloadProgress(pct);
      const next = Math.max(lastIslandProgressRef.current, pct);
      lastIslandProgressRef.current = next;
      toast.updatePersistent(UPDATE_ISLAND_ID, { progress: next }, false);
    });
    return () => {
      unlisten.then((fn) => fn());
      unlistenAccel.then((fn) => fn());
    };
  }, [toast]);

  // 重启安装：路径从 ref 读取，供灵动岛内 onClick 闭包获取最新值（不捕获过期 state）
  const handleInstall = useCallback(async () => {
    const filePath = downloadedFilePathRef.current;
    if (!filePath) return;
    try {
      await invoke("clear_pending_install");
      await invoke("install_update", {
        filePath,
      });
    } catch (error) {
      console.error("Failed to install:", error);
      toast({
        title: t("settings.aboutSettings.installFailed") || "安装失败",
        status: "error",
        duration: 2000,
        isClosable: true,
      });
    }
  }, [t, toast]);

  // 核心下载逻辑（静默与手动共用，互斥进行）
  // manual=true 表示用户手动点击下载：始终允许，不受静默开关限制
  const startDownload = useCallback(
    async (release: ReleaseInfo, openModalOnStart: boolean, manual = false) => {
      // 商店版无应用内更新：一律不下载（版本由微软商店更新）
      if (IS_STORE_BUILD) return;
      if (downloadStartedRef.current) return;
      // 硬门禁：自动下载（manual=false）在静默更新关闭时拒绝；手动下载始终允许
      if (!manual && !autoUpdateEnabledRef.current) {
        console.warn("[Update] 静默更新已关闭，拒绝自动下载");
        return;
      }
      downloadStartedRef.current = true;
      downloadCancelledRef.current = false;
      try {
        await invoke("reset_download_cancel");
      } catch (error) {
        console.error("Failed to reset download cancel flag:", error);
      }
      setLatestRelease(release);
      setHasUpdate(true);
      setIsDownloading(true);
      targetProgressRef.current = 0;
      displayProgressRef.current = 0;
      setDownloadProgress(0);
      setDisplayProgress(0);
      setIsDownloadComplete(false);
      if (openModalOnStart) setIsModalOpen(true);

      // 重置岛内进度/完成标记
      lastIslandProgressRef.current = 0;
      isDownloadCompleteRef.current = false;

      // 灵动岛显示更新下载基线（持久，不自动关闭）
      toast.showPersistent({
        id: UPDATE_ISLAND_ID,
        iconKey: "download",
        status: "blue",
        persistent: true,
        title: t("settings.aboutSettings.islandDownloading") || "新版本下载",
        description: release.tag_name,
        progress: 0,
      });

      try {
        // 优先便携版 ZIP（解压覆盖更新），其次才是 MSI / EXE 安装包
        const zipAssets = release.assets.filter((a) => /\.zip$/i.test(a.name));
        const asset =
          zipAssets.find((a) => /portable/i.test(a.name)) ??
          zipAssets[0] ??
          release.assets.find((a) => /\.(msi|exe)$/i.test(a.name));
        if (asset) {
          const downloadUrl = asset.browser_download_url;
          // 根据文件大小自动选择线程数（16-256）
          const sizeMB = asset.size / (1024 * 1024);
          const maxSegments = Math.min(256, Math.max(16, Math.round(sizeMB / 5) * 16));
          // 获取 exe 所在目录，下载更新包到同目录
          const exeDir = await invoke<string>("get_exe_dir");
          // 调用内置下载器（加速器）下载更新包
          const task = await invoke<{
            id: string;
            fileName: string;
            saveDir: string;
            totalBytes: number;
            downloadedBytes: number;
            status: number;
            errorMessage: string;
          }>("accel_start", {
            url: downloadUrl,
            fileName: asset.name,
            saveDir: exeDir,
            maxSegments,
          });
          accelTaskIdRef.current = task.id;
          // 轮询等待下载完成
          const filePath = await new Promise<string>((resolve, reject) => {
            const timeout = setTimeout(() => reject(new Error("更新下载超时")), 10 * 60 * 1000);
            const checkCompleted = setInterval(async () => {
              try {
                const list = await invoke<Array<{
                  id: string;
                  status: number;
                  errorMessage: string;
                  saveDir: string;
                  fileName: string;
                  totalBytes: number;
                  downloadedBytes: number;
                }>>("accel_list");
                const cur = list.find((t) => t.id === task.id);
                if (cur) {
                  // 轮询时同步更新进度
                  if (cur.totalBytes > 0 && !isDownloadCompleteRef.current) {
                    const pct = (cur.downloadedBytes / cur.totalBytes) * 100;
                    targetProgressRef.current = pct;
                    setDownloadProgress(pct);
                    const next = Math.max(lastIslandProgressRef.current, pct);
                    lastIslandProgressRef.current = next;
                    toast.updatePersistent(UPDATE_ISLAND_ID, { progress: next }, false);
                  }
                  if (cur.status === 3) {
                    clearTimeout(timeout);
                    clearInterval(checkCompleted);
                    resolve(`${cur.saveDir}/${cur.fileName}`);
                  } else if (cur.status === 4) {
                    clearTimeout(timeout);
                    clearInterval(checkCompleted);
                    reject(new Error(cur.errorMessage || "下载失败"));
                  }
                }
              } catch {}
            }, 500);
          });
          // 若下载期间用户关闭了静默更新：删除文件并静默重置
          if (downloadCancelledRef.current) {
            try { await invoke("delete_download_file", { filePath }); } catch {}
            toast.closePersistent(UPDATE_ISLAND_ID);
            downloadStartedRef.current = false;
            setIsDownloading(false);
            setIsDownloadComplete(false);
            setDownloadedFilePath("");
            setDownloadProgress(0);
            setDisplayProgress(0);
            targetProgressRef.current = 0;
            displayProgressRef.current = 0;
            return;
          }
          setDownloadedFilePath(filePath);
          await new Promise((r) => setTimeout(r, 1000));
          targetProgressRef.current = 100;
          setDownloadProgress(100);
          isDownloadCompleteRef.current = true;
          setIsDownloadComplete(true);
          setIsDownloading(false);
          toast.updatePersistent(UPDATE_ISLAND_ID, {
            title: manual
              ? t("settings.aboutSettings.islandClickRestart") || "点击重启"
              : t("settings.aboutSettings.islandAutoApply") || "退出后自动更新",
            status: "success",
            progress: undefined,
            onClick: handleInstall,
          });
          if (autoUpdateEnabledRef.current) {
            try { await invoke("mark_pending_install", { filePath }); } catch {}
          } else if (!manual) {
            try { await invoke("delete_download_file", { filePath }); } catch {}
            toast.closePersistent(UPDATE_ISLAND_ID);
            setIsDownloadComplete(false);
            setDownloadedFilePath("");
          }
        } else {
          toast.closePersistent(UPDATE_ISLAND_ID);
          downloadStartedRef.current = false;
          setIsDownloading(false);
          // 不再跳转 GitHub：没有自动更新包时明确提示，避免用户被带到浏览器
          toast({
            title: t("settings.aboutSettings.noUpdateAsset") || "该版本暂未提供自动更新包",
            description: t("settings.aboutSettings.noUpdateAssetHint") || "请稍后重试，或前往 Releases 手动下载",
            status: "warning",
            duration: 4000,
            isClosable: true,
          });
        }
      } catch (error) {
        // 静默更新已关闭（用户主动取消 / 后端拒绝）：静默重置，绝不提示"下载失败"
        if (downloadCancelledRef.current || !autoUpdateEnabledRef.current) {
          toast.closePersistent(UPDATE_ISLAND_ID);
          downloadStartedRef.current = false;
          setIsDownloading(false);
          setDownloadProgress(0);
          setDisplayProgress(0);
          targetProgressRef.current = 0;
          displayProgressRef.current = 0;
          return;
        }
        console.error("Failed to download:", error);
        toast({
          title: t("settings.aboutSettings.downloadFailed") || "下载失败",
          status: "error",
          duration: 2000,
          isClosable: true,
        });
        toast.closePersistent(UPDATE_ISLAND_ID);
        setIsDownloading(false);
        downloadStartedRef.current = false;
      }
    },
    [t, toast, handleInstall],
  );

  // 启动完成后的自动检查：开启静默 → 后台下载不弹窗；关闭 → 弹窗但不下载
  // 仅主窗口执行：托盘菜单/桌面歌词/悬浮框等独立窗口同样挂载本 Provider，
  // 若各自下载会争抢同一个 Downloads 目标路径（File::create 互相截断，
  // 且校验失败的窗口会 remove_file 删掉别窗已下好的包），导致「点击重启」时文件已不存在。
  useEffect(() => {
    // 商店版不自动检查更新（版本由微软商店更新）
    if (IS_STORE_BUILD) return;
    if (getCurrentWindow().label !== "main") return;
    if (!isStartupComplete) return;
    if (!autoUpdateLoaded) return;
    if (downloadStartedRef.current) return;

    (async () => {
      try {
        const release = await fetchLatestRelease();
        if (release) {
          const found = compareVersions(CURRENT_VERSION, release.tag_name);
          if (found) {
            setLatestRelease(release);
            setHasUpdate(true);
            if (autoUpdateEnabledRef.current) {
              await startDownload(release, false);
            } else {
              // 静默更新关闭：仅弹窗通知，不下载
              setIsModalOpen(true);
            }
          }
        }
      } catch (error) {
        console.error("[Update] 启动更新检查失败:", error);
      }
    })();
  }, [isStartupComplete, autoUpdateLoaded, startDownload]);

  // 托盘"检查更新"事件
  useEffect(() => {
    // 商店版无应用内更新：托盘「检查更新」不生效
    if (IS_STORE_BUILD) return;
    const unlisten = listen("check-update", async () => {
      // 仅主窗口跳转关于页；托盘菜单窗口也挂载了本 Provider，
      // 若不拦截会导致托盘菜单被导航到设置页（下次打开变成主窗口内容）。
      if (getCurrentWindow().label !== "main") return;

      navigate("/settings?section=about");

      setTimeout(async () => {
        // 等待静默更新设置加载完成，避免竞态误用默认值（true）
        while (!autoUpdateLoadedRef.current) {
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        // 静默更新已关闭：托盘检查不再下载、不再弹窗，仅提示已关闭
        if (!autoUpdateEnabledRef.current) {
          toast({
            title: t("settings.generalSettings.autoUpdate") || "自动静默更新",
            description: t("settings.aboutSettings.autoUpdateDisabledHint") || "已关闭，请开启后自动更新",
            status: "info",
            duration: 2000,
            isClosable: true,
          });
          return;
        }
        if (downloadStartedRef.current) {
          setIsModalOpen(true);
          return;
        }
        try {
          const release = await fetchLatestRelease();
          if (release) {
            const found = compareVersions(CURRENT_VERSION, release.tag_name);
            if (found) {
              setLatestRelease(release);
              setHasUpdate(true);
              if (autoUpdateEnabledRef.current) {
                await startDownload(release, false);
                setIsModalOpen(true);
              } else {
                setIsModalOpen(true);
              }
            } else {
              toast({
                title: t("settings.aboutSettings.noUpdate") || "已是最新版本",
                status: "success",
                duration: 2000,
                isClosable: true,
              });
            }
          }
        } catch (error) {
          console.error("Failed to check for updates:", error);
          toast({
            title: t("settings.aboutSettings.checkFailed") || "检查失败",
            status: "error",
            duration: 2000,
            isClosable: true,
          });
        }
      }, 500);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [navigate, toast, t, startDownload]);

  // 手动检查（设置页按钮）：开启静默且有新版 → 自动下载并打开弹窗显示进度
  const handleCheckUpdate = useCallback(async () => {
    // 商店版不检查更新（版本由微软商店更新）
    if (IS_STORE_BUILD) return;
    if (isDownloading || isDownloadComplete) {
      setIsModalOpen(true);
      return;
    }
    setIsChecking(true);
    try {
      const release = await fetchLatestRelease();
      if (release) {
        const found = compareVersions(CURRENT_VERSION, release.tag_name);
        if (found) {
          if (autoUpdateEnabledRef.current) {
            await startDownload(release, true);
          } else {
            setLatestRelease(release);
            setHasUpdate(true);
            setIsModalOpen(true);
          }
        } else {
          toast({
            title: t("settings.aboutSettings.noUpdate") || "已是最新版本",
            status: "success",
            duration: 2000,
            isClosable: true,
          });
        }
      } else {
        toast({
          title: t("settings.aboutSettings.checkFailed") || "检查失败",
          status: "error",
          duration: 2000,
          isClosable: true,
        });
      }
    } catch (error) {
      console.error("Failed to check for updates:", error);
      toast({
        title: t("settings.aboutSettings.checkFailed") || "检查失败",
        status: "error",
        duration: 2000,
        isClosable: true,
      });
    } finally {
      setIsChecking(false);
    }
  }, [isDownloading, isDownloadComplete, startDownload, t, toast]);

  // 弹窗"下载"按钮：用户手动点击，无论静默开关是否开启都允许下载
  const manualDownload = useCallback(async () => {
    if (isDownloading || isDownloadComplete) return;
    let release = latestRelease;
    if (!release) {
      release = await fetchLatestRelease();
    }
    if (!release) {
      toast({
        title: t("settings.aboutSettings.checkFailed") || "检查失败",
        status: "error",
        duration: 2000,
        isClosable: true,
      });
      return;
    }
    const found = compareVersions(CURRENT_VERSION, release.tag_name);
    if (!found) {
      toast({
        title: t("settings.aboutSettings.noUpdate") || "已是最新版本",
        status: "success",
        duration: 2000,
        isClosable: true,
      });
      return;
    }
    await startDownload(release, false, true);
  }, [isDownloading, isDownloadComplete, latestRelease, startDownload, t, toast]);

  const handleSkip = useCallback(async () => {
    // 关闭灵动岛更新下载岛
    toast.closePersistent(UPDATE_ISLAND_ID);
    if (downloadedFilePath) {
      try {
        await invoke("delete_download_file", {
          filePath: downloadedFilePath,
        });
      } catch (error) {
        console.error("Failed to delete file:", error);
      }
    }
    try {
      await invoke("clear_pending_install");
    } catch (error) {
      console.error("Failed to clear pending install:", error);
    }
    downloadStartedRef.current = false;
    setIsModalOpen(false);
    setIsDownloadComplete(false);
    setDownloadedFilePath("");
    setIsDownloading(false);
    setLatestRelease(null);
    setHasUpdate(false);
  }, [downloadedFilePath, toast]);

  const openModal = useCallback(() => setIsModalOpen(true), []);
  const closeModal = useCallback(() => setIsModalOpen(false), []);
  const toggleModal = useCallback(() => setIsModalOpen((v) => !v), []);

  const value = useMemo<UpdateContextValue>(
    () => ({
      latestRelease,
      hasUpdate,
      isChecking,
      isDownloading,
      downloadProgress: displayProgress,
      isDownloadComplete,
      downloadedFilePath,
      isModalOpen,
      autoUpdateEnabled,
      setAutoUpdateEnabled,
      openModal,
      closeModal,
      toggleModal,
      handleCheckUpdate,
      manualDownload,
      handleInstall,
      handleSkip,
    }),
    [
      latestRelease,
      hasUpdate,
      isChecking,
      isDownloading,
      displayProgress,
      isDownloadComplete,
      downloadedFilePath,
      isModalOpen,
      autoUpdateEnabled,
      setAutoUpdateEnabled,
      openModal,
      closeModal,
      toggleModal,
      handleCheckUpdate,
      manualDownload,
      handleInstall,
      handleSkip,
    ],
  );

  return <UpdateContext.Provider value={value}>{children}</UpdateContext.Provider>;
}
