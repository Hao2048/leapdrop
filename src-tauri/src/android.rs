// Android 专属：Wi-Fi 组播锁（MulticastLock）
// Android 为省电默认丢弃组播包，不持锁通常收不到 mDNS 发现广播。
// 通过 wry 初始化的 ndk-context 拿到 JavaVM/Context，调用 WifiManager。
#[cfg(target_os = "android")]
pub fn acquire_multicast_lock() {
    use jni::objects::{JObject, JValue};
    use jni::AttachGuard;
    use std::panic::AssertUnwindSafe;

    fn try_lock() -> Option<()> {
        unsafe {
            let ctx = ndk_context::android_context();
            let vm = jni::JavaVM::from_raw(ctx.vm() as *mut _).ok()?;
            let mut env: AttachGuard<'_> = vm.attach_current_thread().ok()?;
            let context = JObject::from_raw(ctx.context() as *mut _);

            // WifiManager wm = (WifiManager) context.getSystemService("wifi")
            let service = env.new_string("wifi").ok()?;
            let wifi = env
                .call_method(
                    &context,
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[JValue::Object(&service)],
                )
                .ok()?
                .l()
                .ok()?;
            // MulticastLock lock = wm.createMulticastLock("leapdrop")
            let tag = env.new_string("leapdrop").ok()?;
            let lock = env
                .call_method(
                    &wifi,
                    "createMulticastLock",
                    "(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;",
                    &[JValue::Object(&tag)],
                )
                .ok()?
                .l()
                .ok()?;
            // lock.setReferenceCounted(false); lock.acquire();
            env.call_method(&lock, "setReferenceCounted", "(Z)V", &[JValue::Bool(0)])
                .ok()?;
            env.call_method(&lock, "acquire", "()V", &[]).ok()?;
            // 转全局引用并泄漏：进程存续期间锁对象不被 GC、锁不释放
            let global = env.new_global_ref(&lock).ok()?;
            std::mem::forget(global);
            Some(())
        }
    }

    let ok = std::panic::catch_unwind(AssertUnwindSafe(try_lock))
        .unwrap_or(None)
        .is_some();
    if !ok {
        eprintln!("[leapdrop] 组播锁获取失败（部分设备可能收不到 mDNS）");
    }
}
