//! The video stage: a full-bleed letterboxed `<video>` with the
//! annotation layer positioned over the *used* picture box only.
//!
//! Clicking the picture toggles play — except in draw mode, where the
//! stage belongs to the pen: the overlay eats pointer events, strokes
//! author in normalized frame coordinates, and playback stays paused
//! (a drawing is frame-anchored; motion under the pen would lie).

use dioxus::prelude::*;
use files_proto::AnnotationStroke;

use super::{DrawCtx, PlayerCtx, STROKE_WIDTH, normalized_point, points_attr, toggle_play};

/// The stage. `stage_id` wraps the letterbox area [`PlayerCtx`]
/// measures; `video_id` is the element every transport op addresses.
#[component]
pub(crate) fn VideoStage(
    stage_id: String,
    video_id: String,
    src: String,
    /// Compact chrome (the mini player) caps the stage height instead
    /// of flexing to fill.
    mini: bool,
    /// A frame to stand behind the `<video>` until it paints — the
    /// filmstrip sprite, left-cropped to its first tile. Some browsers
    /// leave `preload="metadata"` black until play; the stage should
    /// never be a black box when a frame is already paid for.
    #[props(default)]
    poster: Option<String>,
    /// The peaks rendition URL for an AUDIO file — the stage draws its
    /// waveform instead of frames (the `<video>` element still runs
    /// the transport; an audio/mp4 stream just has nothing to paint).
    #[props(default)]
    waveform: Option<String>,
) -> Element {
    let player = PlayerCtx::use_ctx();
    let mut draw = DrawCtx::use_ctx();

    // The stroke under the pointer right now, in frame coordinates.
    let mut active = use_signal(|| Option::<AnnotationStroke>::None);

    // The audio waveform: fetch the peaks PCM (raw mono s16le), reduce
    // to one max-amplitude bar per column, draw once. Pure JS because
    // the canvas and the bytes both live on that side of the seam.
    let wave_id = use_hook(|| format!("review-wave-{}", uuid::Uuid::new_v4().simple()));
    {
        let wave_id = wave_id.clone();
        let waveform = waveform.clone();
        use_effect(use_reactive!(|(waveform,)| {
            if let Some(url) = waveform {
                let js = format!(
                    "(async function(){{\
                       var c=document.getElementById('{wave_id}');if(!c)return;\
                       try{{\
                         var r=await fetch('{url}');if(!r.ok)return;\
                         var d=new Int16Array(await r.arrayBuffer());\
                         var W=c.width=c.clientWidth*2,H=c.height=c.clientHeight*2;\
                         var x=c.getContext('2d');if(!x)return;\
                         x.clearRect(0,0,W,H);\
                         var step=6,n=Math.max(1,Math.floor(W/step)),per=Math.max(1,Math.floor(d.length/n));\
                         x.fillStyle='rgba(129,140,248,0.85)';\
                         for(var i=0;i<n;i++){{\
                           var m=0,s=i*per,e=Math.min(d.length,s+per);\
                           for(var j=s;j<e;j++){{var v=Math.abs(d[j]);if(v>m)m=v;}}\
                           var h=Math.max(3,m/32768*H*0.85);\
                           x.fillRect(i*step,(H-h)/2,step*0.6,h);\
                         }}\
                       }}catch(e){{}}\
                     }})();"
                );
                let _ = dioxus::document::eval(&js);
            }
        }));
    }

    // Resume where you left off: position persists per rendition file
    // (the URL minus its grant token — stable per version, so a new
    // version starts fresh) and clears when playback reaches the end.
    // Lives on the element via JS because the element owns the clock;
    // the key rides `dataset` so a version swap on the same mount
    // re-targets the save without stacking listeners.
    {
        let video_id = video_id.clone();
        let key = format!(
            "task.review.pos.{}",
            src.split('?').next().unwrap_or(src.as_str())
        );
        use_effect(use_reactive!(|(key,)| {
            let js = format!(
                "(function(){{\
                   var v=document.getElementById('{video_id}');if(!v)return;\
                   v.dataset.taskResumeKey='{key}';\
                   var r=function(){{try{{\
                     var p=parseFloat(localStorage.getItem(v.dataset.taskResumeKey)||'0');\
                     if(p>1&&v.duration&&p<v.duration-2){{v.currentTime=p;}}\
                   }}catch(e){{}}}};\
                   if(v.readyState>=1){{r();}}else{{v.addEventListener('loadedmetadata',r,{{once:true}});}}\
                   if(!v.dataset.taskResumeWired){{v.dataset.taskResumeWired='1';\
                     v.addEventListener('timeupdate',function(){{try{{\
                       if(!v.paused&&v.currentTime>1){{localStorage.setItem(v.dataset.taskResumeKey,String(v.currentTime));}}\
                     }}catch(e){{}}}});\
                     v.addEventListener('ended',function(){{try{{\
                       localStorage.removeItem(v.dataset.taskResumeKey);\
                     }}catch(e){{}}}});\
                   }}\
                 }})();"
            );
            let _ = dioxus::document::eval(&js);
        }));
    }

    let (fx, fy, fw, fh) = (player.frame_rect)();
    let frame_style = format!("left:{fx}px;top:{fy}px;width:{fw}px;height:{fh}px;");
    // Screen-space stroke width: normalized width × rendered frame
    // width, floored so hairlines stay visible on small stages.
    let stroke_px = (f64::from(STROKE_WIDTH) * fw).max(2.0);

    let overlay_visible = *draw.draw_mode.read()
        || !draw.pending.read().is_empty()
        || !draw.viewing.read().is_empty()
        || active.read().is_some();

    rsx! {
        div {
            id: stage_id.clone(),
            class: if mini {
                "relative w-full aspect-video max-h-96 bg-black overflow-hidden rounded-md"
            } else {
                "relative flex-1 min-h-0 bg-black overflow-hidden"
            },
            // Gone the moment the clock moves: from then on the video
            // has painted, and a backdrop would only leak through the
            // letterbox bars.
            if let Some(p) = poster.as_ref().filter(|_| (player.now)() <= 0.05) {
                div {
                    class: "absolute inset-0 pointer-events-none",
                    style: "background-image:url('{p}');background-size:cover;background-position:left center;background-repeat:no-repeat;",
                }
            }
            if waveform.is_some() {
                canvas {
                    id: wave_id.clone(),
                    class: "absolute inset-y-0 left-6 right-6 h-full pointer-events-none",
                }
            }
            video {
                id: video_id.clone(),
                src,
                preload: "metadata",
                playsinline: true,
                class: if *draw.draw_mode.read() {
                    "absolute inset-0 w-full h-full object-contain pointer-events-none"
                } else {
                    "absolute inset-0 w-full h-full object-contain cursor-pointer"
                },
                onclick: {
                    let video_id = video_id.clone();
                    move |_| {
                        if !*draw.draw_mode.peek() {
                            toggle_play(&video_id);
                        }
                    }
                },
                // Starting playback clears a displayed drawing — it is
                // anchored to the frame it was drawn on (reference rule).
                onplay: move |_| {
                    draw.clear_focus();
                },
            }
            // Annotation layer, sized to the used picture box — never
            // the letterbox bars (normalized coordinates stay
            // meaningful across renditions and window sizes).
            if overlay_visible && fw > 0.0 {
                svg {
                    class: if *draw.draw_mode.read() {
                        "absolute cursor-crosshair touch-none"
                    } else {
                        "absolute pointer-events-none"
                    },
                    style: frame_style,
                    view_box: "0 0 1 1",
                    preserve_aspect_ratio: "none",
                    onpointerdown: move |evt: Event<PointerData>| {
                        if !*draw.draw_mode.peek() {
                            return;
                        }
                        let c = evt.data().element_coordinates();
                        // Pointer coords are overlay-relative; the
                        // overlay IS the frame rect, so normalize
                        // against its own size.
                        let (_, _, w, h) = *player.frame_rect.peek();
                        if let Some(p) = normalized_point(c.x, c.y, (0.0, 0.0, w, h)) {
                            // The drawing belongs to the frame its first
                            // stroke landed on — record it once.
                            if draw.pending_at.peek().is_none() {
                                draw.pending_at.set(Some(*player.now.peek()));
                            }
                            active.set(Some(AnnotationStroke {
                                points: vec![p],
                                color: (*draw.color.peek()).into(),
                                width: STROKE_WIDTH,
                            }));
                        }
                    },
                    onpointermove: move |evt: Event<PointerData>| {
                        if active.peek().is_none() {
                            return;
                        }
                        let c = evt.data().element_coordinates();
                        let (_, _, w, h) = *player.frame_rect.peek();
                        if let Some(p) = normalized_point(c.x, c.y, (0.0, 0.0, w, h)) {
                            if let Some(stroke) = active.write().as_mut() {
                                stroke.points.push(p);
                            }
                        }
                    },
                    onpointerup: move |_| {
                        if let Some(stroke) = active.take() {
                            if stroke.points.len() > 1 {
                                draw.pending.write().push(stroke);
                            }
                        }
                    },
                    onpointerleave: move |_| {
                        if let Some(stroke) = active.take() {
                            if stroke.points.len() > 1 {
                                draw.pending.write().push(stroke);
                            }
                        }
                    },
                    for stroke in draw.viewing.read().iter()
                        .chain(draw.pending.read().iter())
                        .chain(active.read().iter())
                    {
                        polyline {
                            points: points_attr(stroke),
                            fill: "none",
                            stroke: stroke.color.clone(),
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            // Non-scaling stroke: the viewBox is
                            // stretched non-uniformly, so width must be
                            // screen-space.
                            stroke_width: "{stroke_px}",
                            "vector-effect": "non-scaling-stroke",
                            // Never a pointer target: a pointerdown over
                            // an existing stroke must report
                            // overlay-relative coordinates.
                            pointer_events: "none",
                        }
                    }
                }
            }
        }
    }
}
