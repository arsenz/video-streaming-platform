const fileInput = document.getElementById('fileInput');
const dropZone = document.getElementById('dropZone');
const uploadSection = document.getElementById('uploadSection');
const statusSection = document.getElementById('statusSection');
const playerSection = document.getElementById('playerSection');
const uploadProgressContainer = document.getElementById('uploadProgressContainer');
const uploadProgressBar = document.getElementById('uploadProgressBar');
const uploadStatusText = document.getElementById('uploadStatusText');
const jobStatusText = document.getElementById('jobStatusText');

const R2_PUBLIC_BASE = 'http://localhost:4566/video-uploads';

// Drag & Drop Styling
dropZone.addEventListener('dragover', (e) => {
    e.preventDefault();
    dropZone.classList.add('dragover');
});
dropZone.addEventListener('dragleave', () => dropZone.classList.remove('dragover'));
dropZone.addEventListener('drop', (e) => {
    e.preventDefault();
    dropZone.classList.remove('dragover');
    if (e.dataTransfer.files.length > 0) {
        handleUpload(e.dataTransfer.files[0]);
    }
});
fileInput.addEventListener('change', (e) => {
    if (e.target.files.length > 0) {
        handleUpload(e.target.files[0]);
    }
});

async function handleUpload(file) {
    if (!file.type.startsWith('video/')) {
        alert('Please select a video file.');
        return;
    }

    try {
        //  Get Presigned URL
        uploadStatusText.innerText = "Requesting upload URL...";
        const res = await fetch('/api/upload/url');
        const data = await res.json();

        if (!data.upload_url || !data.video_id) throw new Error("Invalid URL response");

        // UI Updates
        dropZone.classList.add('hidden');
        uploadProgressContainer.classList.remove('hidden');
        uploadStatusText.innerText = "Uploading directly to storage...";

        //  Perform PUT Request tracking progress via XMLHttpRequest
        await new Promise((resolve, reject) => {
            const xhr = new XMLHttpRequest();
            xhr.open('PUT', data.upload_url, true);

            xhr.upload.onprogress = (e) => {
                if (e.lengthComputable) {
                    const percentComplete = (e.loaded / e.total) * 100;
                    uploadProgressBar.style.width = percentComplete + '%';
                }
            };

            xhr.onload = () => {
                if (xhr.status >= 200 && xhr.status < 300) {
                    resolve();
                } else {
                    reject(new Error("Upload failed"));
                }
            };
            xhr.onerror = () => reject(new Error("Network error"));

            xhr.send(file);
        });

        uploadStatusText.innerText = "Upload complete! Triggers processing...";

        // Emulate R2 Webhook trigger manually for localstack because localstack S3 event bridge 
        // to our API requires explicit wiring which might not be fully configured in init script
        try {
            uploadStatusText.innerText = "Simulating Cloudflare Webhook...";
            await fetch('/api/webhook/r2', {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({
                    file_name: `${data.video_id}.mp4`,
                    size_bytes: file.size
                })
            });
        } catch (e) {
            console.error("Webhook trigger failed, assuming automatic hook active", e);
        }

        // Move to polling
        pollStatus(data.video_id);

    } catch (error) {
        console.error(error);
        uploadStatusText.innerText = "Error: " + error.message;
        dropZone.classList.remove('hidden');
        uploadProgressContainer.classList.add('hidden');
    }
}

async function pollStatus(videoId) {
    uploadSection.classList.add('hidden');
    statusSection.classList.remove('hidden');

    const interval = setInterval(async () => {
        try {
            const res = await fetch('/api/status/' + videoId);
            if (!res.ok) return;

            const data = await res.json();

            if (data.status === 'processing') {
                jobStatusText.innerText = "Segmenting & Parallel Transcoding in progress...";
            } else if (data.status === 'ready') {
                clearInterval(interval);
                showPlayer(videoId);
            }
        } catch (e) {
            console.error('Polling error', e);
        }
    }, 2000);
}

function showPlayer(videoId) {
    statusSection.classList.add('hidden');
    playerSection.classList.remove('hidden');

    const videoElement = document.getElementById('videoPlayer');
    // M3U8 Master URL in S3
    const videoSrc = `${R2_PUBLIC_BASE}/transcoded/${videoId}/master.m3u8`;

    if (Hls.isSupported()) {
        const hls = new Hls({
            // Tweaks for smoother playback
            lowLatencyMode: true,
        });
        hls.loadSource(videoSrc);
        hls.attachMedia(videoElement);
        hls.on(Hls.Events.MANIFEST_PARSED, function () {
            console.log("Manifest parsed! Quality levels:", hls.levels.map(l => l.height + "p"));
        });
        // Auto play
        videoElement.play().catch(() => { });
    } else if (videoElement.canPlayType('application/vnd.apple.mpegurl')) {
        // Native Apple Safari fallback
        videoElement.src = videoSrc;
        videoElement.play();
    } else {
        alert("Your browser does not support HLS.");
    }
}
