//! A screen share's sound: everything the computer plays except Hermes itself, so the call is
//! not sent back to the people in it. On Windows this is the system's process loopback
//! (Windows 10 version 2004 and later); elsewhere there is no share sound yet.

use hermes_client_core::audio::mixer::STEREO_FRAME;

/// What a share sound frame is given to: the Opus packet and its level (RMS, full scale 1).
pub type Send = Box<dyn FnMut(&[u8], f32) + std::marker::Send + 'static>;

/// Cuts interleaved stereo samples into 20 ms frames.
#[derive(Default)]
pub struct Framer {
    pending: Vec<f32>,
}

impl Framer {
    /// Adds samples; `each` gets every whole frame that is ready.
    pub fn push(&mut self, samples: &[f32], mut each: impl FnMut(&[f32; STEREO_FRAME])) {
        self.pending.extend_from_slice(samples);
        let whole = self.pending.len() / STEREO_FRAME * STEREO_FRAME;
        for frame in self.pending[..whole].as_chunks::<STEREO_FRAME>().0 {
            each(frame);
        }
        self.pending.drain(..whole);
    }
}

#[cfg(windows)]
pub use imp::ShareSound;

#[cfg(not(windows))]
pub struct ShareSound;

#[cfg(not(windows))]
impl ShareSound {
    pub fn start(_send: Send) -> Result<ShareSound, String> {
        Err("Sharing sound needs Windows".into())
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use hermes_client_core::audio::codec::ShareEncoder;
    use hermes_client_core::audio::dsp;
    use std::ffi::c_void;
    use std::sync::atomic::AtomicU32;
    use windows::Win32::Media::Audio::{
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
        ActivateAudioInterfaceAsync, IActivateAudioInterfaceCompletionHandler,
        IActivateAudioInterfaceCompletionHandler_Vtbl, IAudioCaptureClient, IAudioClient,
        PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        WAVEFORMATEX,
    };
    use windows::Win32::System::Com::StructuredStorage::{
        PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
    };
    use windows::Win32::System::Com::{BLOB, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::System::Variant::VT_BLOB;

    use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface};

    /// `WAVE_FORMAT_IEEE_FLOAT` (in a part of the Windows API not otherwise needed).
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const E_NOINTERFACE: HRESULT = HRESULT(0x8000_4002_u32 as i32);
    /// `IAgileObject`: the handler may be called from any thread.
    const IID_IAGILE_OBJECT: GUID = GUID::from_u128(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90);

    use super::{Framer, Send};

    /// Captures until dropped.
    pub struct ShareSound {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl ShareSound {
        /// Starts capturing; `Err` when this Windows has no process loopback.
        pub fn start(send: Send) -> Result<ShareSound, String> {
            let stop = Arc::new(AtomicBool::new(false));
            let (ready_tx, ready_rx) = mpsc::channel();
            let stopping = stop.clone();
            let thread = std::thread::Builder::new()
                .name("share-sound".into())
                .spawn(move || run(send, &stopping, &ready_tx))
                .map_err(|e| e.to_string())?;
            match ready_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(())) => Ok(ShareSound {
                    stop,
                    thread: Some(thread),
                }),
                Ok(Err(err)) => Err(err),
                Err(_) => {
                    stop.store(true, Ordering::Relaxed);
                    Err("the sound did not start".into())
                }
            }
        }
    }

    impl Drop for ShareSound {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// Tells the waiting thread that activation finished: a small COM object made by hand (the
    /// `windows` crate's `implement` needs another crate).
    #[repr(C)]
    struct Activated {
        vtable: *const IActivateAudioInterfaceCompletionHandler_Vtbl,
        refs: AtomicU32,
        done: std::sync::Mutex<mpsc::Sender<()>>,
    }

    static ACTIVATED_VTABLE: IActivateAudioInterfaceCompletionHandler_Vtbl =
        IActivateAudioInterfaceCompletionHandler_Vtbl {
            base__: IUnknown_Vtbl {
                QueryInterface: activated_query,
                AddRef: activated_add_ref,
                Release: activated_release,
            },
            ActivateCompleted: activated_completed,
        };

    unsafe extern "system" fn activated_query(
        this: *mut c_void,
        iid: *const GUID,
        out: *mut *mut c_void,
    ) -> HRESULT {
        // SAFETY: Windows passes a valid IID and out pointer, and `this` is an `Activated`.
        unsafe {
            let iid = *iid;
            if iid == IUnknown::IID
                || iid == IActivateAudioInterfaceCompletionHandler::IID
                || iid == IID_IAGILE_OBJECT
            {
                activated_add_ref(this);
                *out = this;
                HRESULT(0)
            } else {
                *out = std::ptr::null_mut();
                E_NOINTERFACE
            }
        }
    }

    unsafe extern "system" fn activated_add_ref(this: *mut c_void) -> u32 {
        // SAFETY: `this` is a live `Activated`.
        unsafe {
            (*this.cast::<Activated>())
                .refs
                .fetch_add(1, Ordering::AcqRel)
                + 1
        }
    }

    unsafe extern "system" fn activated_release(this: *mut c_void) -> u32 {
        // SAFETY: `this` came from `Box::into_raw` and is freed with the last reference.
        unsafe {
            let left = (*this.cast::<Activated>())
                .refs
                .fetch_sub(1, Ordering::AcqRel)
                - 1;
            if left == 0 {
                drop(Box::from_raw(this.cast::<Activated>()));
            }
            left
        }
    }

    unsafe extern "system" fn activated_completed(
        this: *mut c_void,
        _operation: *mut c_void,
    ) -> HRESULT {
        // SAFETY: `this` is a live `Activated`.
        let done = unsafe { &(*this.cast::<Activated>()).done };
        let _ = done.lock().expect("activated").send(());
        HRESULT(0)
    }

    fn activated(done: mpsc::Sender<()>) -> IActivateAudioInterfaceCompletionHandler {
        let object = Box::into_raw(Box::new(Activated {
            vtable: &raw const ACTIVATED_VTABLE,
            refs: AtomicU32::new(1),
            done: std::sync::Mutex::new(done),
        }));
        // SAFETY: a COM object with one reference, which the interface now owns.
        unsafe { IActivateAudioInterfaceCompletionHandler::from_raw(object.cast()) }
    }

    /// 48 kHz stereo float: the format the share sound is sent in, so Windows converts to it.
    fn format() -> WAVEFORMATEX {
        WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 48_000 * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        }
    }

    /// An audio client for everything but this process and its children.
    unsafe fn loopback_client() -> windows::core::Result<IAudioClient> {
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: unsafe { GetCurrentProcessId() },
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        // Never dropped: clearing a PROPVARIANT frees its blob, which here is `params` on the
        // stack.
        let variant = std::mem::ManuallyDrop::new(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: std::ptr::from_ref(&params).cast_mut().cast(),
                        },
                    },
                }),
            },
        });
        let (done_tx, done_rx) = mpsc::channel();
        let handler = activated(done_tx);
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&raw const *variant),
                &handler,
            )?
        };
        if done_rx.recv_timeout(Duration::from_secs(3)).is_err() {
            return Err(windows::core::Error::from(HRESULT(0x8000_000E_u32 as i32)));
        }
        let mut result = HRESULT(0);
        let mut unknown: Option<IUnknown> = None;
        unsafe { operation.GetActivateResult(&mut result, &mut unknown)? };
        result.ok()?;
        unknown
            .ok_or_else(|| windows::core::Error::from(HRESULT(0x8000_4005_u32 as i32)))?
            .cast()
    }

    fn run(mut send: Send, stop: &AtomicBool, ready: &mpsc::Sender<Result<(), String>>) {
        // SAFETY: COM on this thread only, undone at the end; every pointer below comes from
        // Windows for the call it is used in.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let started = (|| -> windows::core::Result<(IAudioClient, IAudioCaptureClient)> {
                let client = loopback_client()?;
                let format = format();
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    200_000,
                    0,
                    &format,
                    None,
                )?;
                let capture: IAudioCaptureClient = client.GetService()?;
                client.Start()?;
                Ok((client, capture))
            })();
            let (client, capture) = match started {
                Ok(pair) => pair,
                Err(err) => {
                    let _ = ready.send(Err(format!(
                        "this Windows can't share sound ({})",
                        err.message()
                    )));
                    CoUninitialize();
                    return;
                }
            };
            let _ = ready.send(Ok(()));
            let mut framer = Framer::default();
            let mut encoder = ShareEncoder::default();
            let mut samples: Vec<f32> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                while capture.GetNextPacketSize().is_ok_and(|n| n > 0) {
                    let mut data = std::ptr::null_mut();
                    let (mut frames, mut flags) = (0u32, 0u32);
                    if capture
                        .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                        .is_err()
                    {
                        break;
                    }
                    let count = frames as usize * 2;
                    samples.clear();
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                        samples.resize(count, 0.0);
                    } else {
                        samples.extend_from_slice(std::slice::from_raw_parts(
                            data.cast::<f32>(),
                            count,
                        ));
                    }
                    let _ = capture.ReleaseBuffer(frames);
                    framer.push(&samples, |frame| {
                        if let Some(opus) = encoder.encode(frame) {
                            send(opus, dsp::rms(frame));
                        }
                    });
                }
            }
            let _ = client.Stop();
            drop(capture);
            drop(client);
            CoUninitialize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a Windows audio service: `cargo test -p hermes-gui loopback_starts -- --ignored`.
    #[test]
    #[ignore = "needs Windows audio"]
    #[cfg(windows)]
    fn loopback_starts_and_gives_frames() {
        let frames = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counted = frames.clone();
        let sound = ShareSound::start(Box::new(move |opus, _| {
            assert!(!opus.is_empty());
            counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }))
        .expect("process loopback starts");
        std::thread::sleep(std::time::Duration::from_millis(500));
        drop(sound);
        // Even silence is captured as frames.
        assert!(frames.load(std::sync::atomic::Ordering::Relaxed) >= 10);
    }

    #[test]
    fn samples_come_out_in_whole_frames() {
        let mut framer = Framer::default();
        let mut frames = 0;
        framer.push(&vec![0.5; STEREO_FRAME / 2], |_| frames += 1);
        assert_eq!(frames, 0);
        framer.push(&vec![0.5; STEREO_FRAME * 2], |f| {
            assert_eq!(f[0], 0.5);
            frames += 1;
        });
        assert_eq!(frames, 2);
        assert_eq!(framer.pending.len(), STEREO_FRAME / 2);
    }
}
