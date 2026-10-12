//! 设备档案的启动同步（adopt）：读库 → 阵亡名单/远古快照预筛 → 版本对齐。
//!
//! 从壳的 bootstrap 迁入（双壳共用：Tauri 与 GPUI 的启动装配走同一段设备
//! 引导，「操作一致」的第一层）。纯 store + signer，不走网络——健康探针与
//! 轮换是异步生命周期，留在各壳的 bootstrap（Tauri 侧挂 async_runtime；
//! GPUI 侧待其生命周期装配时接入同一套）。

use hongguo_signer::device::{DeviceProfile, align_app_version, video_device};
use hongguo_store::Store;

/// 已被服务端风控清理、必须从库里淘汰的静态档案 install_id（2026-10-04
/// 实测旧 71332 档案 reading 系整段 0 字节拒）。**今后每次淘汰静态档案，
/// 旧 iid 必须追加进名单**——健康探针只兜「未来的死」，名单兜「已知的死」，
/// 缺补会让老用户再次集体阵亡（2026-10-09 搜索全挂事故的教训）。
pub const RETIRED_STATIC_IIDS: &[&str] = &["1905892595382586"];

/// 轮换元数据（`device_profile` 元数据行，JSON 文档）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BootstrapMeta {
    /// 现役档案来源：registered（注册产物）/ static_fallback（静态兜底）/
    /// adopted（库里继承，尚未复检）。
    #[serde(default)]
    pub source: String,
    /// 最近一次换档前的旧 iid。
    #[serde(default)]
    pub previous_iid: Option<String>,
    /// 上次注册尝试时间（ms epoch；退避判定基准）。
    #[serde(default)]
    pub last_register_attempt_ms: Option<i64>,
    /// 上次注册失败原因（成功清空）。
    #[serde(default)]
    pub last_register_error: Option<String>,
}

pub fn read_meta(store: &Store) -> BootstrapMeta {
    store
        .device_meta_get()
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

pub fn write_meta(store: &Store, meta: &BootstrapMeta) {
    if let Ok(v) = serde_json::to_value(meta)
        && let Err(e) = store.device_meta_set(&v)
    {
        log::warn!("[Device] 元数据落库失败（不影响换档）: {e}");
    }
}

/// 启动同步装配：读库 → 阵亡名单/远古快照预筛 → 版本对齐。
/// 返回 (现役档案, 来源)。不走网络——探针与轮换在壳的 bootstrap 后台任务里。
pub fn adopt_sync(store: &Store) -> (DeviceProfile, &'static str) {
    match store.device_profile() {
        Ok(Some(p)) => {
            let current = video_device();
            let dead_listed = RETIRED_STATIC_IIDS.contains(&p.get("iid"));
            let ancient = p.get("cdid").is_empty();
            if (dead_listed || ancient) && p.get("iid") != current.get("iid") {
                return repave_dead_profile(store, &p, dead_listed);
            }
            let mut p = p;
            align_app_version(&mut p);
            (p, "adopted")
        }
        Ok(None) => {
            let fallback = video_device();
            if let Err(e) = store.save_device_profile(&fallback) {
                log::warn!("[Device] 静态设备档案落库失败（不影响启动）: {e}");
            }
            let mut meta = read_meta(store);
            meta.source = "static_fallback".into();
            write_meta(store, &meta);
            (fallback, "static_fallback")
        }
        Err(e) => {
            log::warn!("[Device] 设备档案读取失败，用静态兜底: {e}");
            (video_device(), "static_fallback")
        }
    }
}

/// 启动预筛命中的重铺：死档案进备份行，静态兜底顶上，元数据记痕迹。
fn repave_dead_profile(
    store: &Store,
    dead: &DeviceProfile,
    dead_listed: bool,
) -> (DeviceProfile, &'static str) {
    let current = video_device();
    log::warn!(
        "[Device] 库内档案已被风控淘汰（iid={}，{}），重铺静态兜底（iid={}）",
        dead.get("iid"),
        if dead_listed {
            "阵亡名单"
        } else {
            "缺 cdid 的远古快照"
        },
        current.get("iid")
    );
    let _ = store.backup_device_profile();
    if let Err(e) = store.save_device_profile(&current) {
        log::warn!("[Device] 重铺落库失败（不影响启动）: {e}");
    }
    let mut meta = read_meta(store);
    meta.source = "static_fallback".into();
    meta.previous_iid = Some(dead.get("iid").to_string());
    write_meta(store, &meta);
    (current, "static_fallback")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hongguo_store::Store;

    fn store() -> Store {
        Store::open_memory().expect("内存库")
    }

    /// 阵亡名单上的库档案：启动同步就重铺静态兜底，旧档进备份行，
    /// 元数据记下轮换痕迹。
    #[test]
    fn adopt_repaves_dead_listed_profile_with_backup() {
        let db = store();
        let mut stale = video_device();
        stale.set("iid", "1905892595382586");
        stale.set("cdid", "babf8a0e-8586-40ca-bb75-20a1a698b43f");
        db.save_device_profile(&stale).expect("写死档案");

        let (profile, source) = adopt_sync(&db);
        assert_eq!(source, "static_fallback");
        assert_eq!(
            profile.get("iid"),
            video_device().get("iid"),
            "重铺为当前兜底"
        );

        let backup = db
            .device_profile_backup()
            .expect("读备份")
            .expect("旧档已留底");
        assert_eq!(backup.get("iid"), "1905892595382586", "备份是旧档");

        let meta = read_meta(&db);
        assert_eq!(meta.source, "static_fallback");
        assert_eq!(meta.previous_iid.as_deref(), Some("1905892595382586"));
    }

    /// 活着的注册产物（有 cdid、iid 不在名单）：adopt 原样放行，只做版本对齐。
    #[test]
    fn adopt_keeps_healthy_registered_profile() {
        let db = store();
        let mut reg = video_device();
        reg.set("iid", "9999999999999999");
        reg.set("device_id", "8888888888888888");
        reg.set("cdid", "0f0f0f0f-0000-4000-8000-000000000000");
        db.save_device_profile(&reg).expect("写注册档案");

        let (profile, source) = adopt_sync(&db);
        assert_eq!(source, "adopted");
        assert_eq!(profile.get("iid"), "9999999999999999");
        assert_eq!(
            profile.get("version_code"),
            video_device().get("version_code"),
            "版本身份对齐到当前客户端"
        );
    }

    /// 库里没档案：落静态兜底。第二次 adopt 起同一兜底不再动（iid 相同不重铺）。
    #[test]
    fn adopt_seeds_static_when_db_empty() {
        let db = store();
        let (profile, source) = adopt_sync(&db);
        assert_eq!(source, "static_fallback");
        assert_eq!(profile.get("iid"), video_device().get("iid"));
        assert!(db.device_profile().expect("读回").is_some());
    }

    /// 元数据行的读写往返（含空→有→覆盖）。
    #[test]
    fn device_meta_roundtrip() {
        let db = store();
        assert!(db.device_meta_get().expect("空").is_none());
        let v = serde_json::json!({"source": "registered", "previous_iid": "1"});
        db.device_meta_set(&v).expect("写");
        assert_eq!(db.device_meta_get().expect("读"), Some(v));
        db.device_meta_set(&serde_json::json!({"source": "static_fallback"}))
            .expect("覆盖");
        assert_eq!(
            db.device_meta_get().expect("读"),
            Some(serde_json::json!({"source": "static_fallback"}))
        );
    }
}
