# https://github.com/Thomvanoorschot/boring_tls
#zig build -Doptimize=ReleaseSafe

# https://pengrl.com/lal/#/RTSPFFPlayBlur
#echo 2000000 > /proc/sys/net/core/rmem_default
#echo 2000000 > /proc/sys/net/core/rmem_max
#  tcpdump -i any -s 0 -w rtsp_capture.pcap port 8554
# tcpdump -i any -s 0 -w rtmp_capture.pcap port 1935
#  tcpdump udp -i any -s 0 -w net_capture.pcap
# tcpdump -i any -s 0 -w net_capture.pcap port 8554 or port 1935 or udp
# tcpdump -i any -s 0 -w sms_capture.pcap port 8554 or port 1935 or udp
# tcpdump -i any -s 0 -w net_capture.pcap port 8080 or port 10001

# ulimit -n 102400
# export SDL_VIDEODRIVER=dummy
# export SDL_AUDIODRIVER=dummy

sudo sysctl -w net.core.rmem_max=2000000
sudo sysctl -w net.core.rmem_default=2000000

#系统级低延迟建议 (针对 nft_do_chain 热点):
#目前的 perf显示内核防火墙损耗占比依然较高（10%）。如果需要极致性能且您的环境允许，建议在宿主机执行以下命令以绕过 RTMP的连接跟踪：
#sudo iptables -t raw -A PREROUTING -p tcp --dport 1935 -j NOTRACK
#sudo iptables -t raw -A OUTPUT -p tcp --sport 1935 -j NOTRACK

ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c copy -f flv rtmp://localhost/live/test &> push.log
ffplay -fflags nobuffer  rtmp://localhost/live/test
gst-launch-1.0 -v \
    videotestsrc is-live=true pattern=ball ! \
    videoconvert ! \
    x264enc tune=zerolatency bitrate=1000 ! \
    video/x-h264,profile=constrained-baseline ! h264parse ! mux. \
    audiotestsrc is-live=true wave=sine ! \
    audioconvert ! \
    voaacenc bitrate=128000 ! aacparse ! mux. \
    flvmux name=mux streamable=true ! \
    rtmp2sink location="rtmp://localhost/live/test"

ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c copy -f rtsp rtsp://127.0.0.1:5544/live/stream
ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c copy -rtsp_transport tcp -f rtsp rtsp://127.0.0.1:5544/live/test
ffmpeg -stream_loop -1 -re -i ./test_media_files/Test\ Jellyfin\ 1080p\ AV1\ 10bit\ 3M.mp4 -c copy -strict experimental -f rtsp -rtsp_transport tcp rtsp://127.0.0.1:8554/live/test


让我建议您在 Windows 上增加探测时间和缓冲区大小：
https://sample.cat/zh/mp4



============== rtsp ffplay 调试命令 ==============
ffplay -rtsp_transport tcp rtsp://127.0.0.1:8554/live/test &> pull.log
ffplay -buffer_size 1000000 -rtsp_transport udp rtsp://127.0.0.1:8554/live/test
ffmpeg -re -stream_loop -1 -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv -c copy -rtsp_transport tcp -f rtsp rtsp://127.0.0.1:8554/live/test
ffplay -fflags nobuffer -flags low_delay -rtsp_transport tcp rtsp://127.0.0.1:8554/live/test

ffmpeg -re -i ./test_media_files/bbb_sunflower_1080p_30fps_normal.flv \
  -c:v libsvtav1 -cpu-used 6 -preset 6 -tune 1 -g 30 -keyint_min 30  -b:v 500K  -usage realtime \
  -c:a copy -strict -2 \
  -rtsp_transport tcp -f rtsp rtsp://127.0.0.1:8554/live/test &> push.log

#sudo apt-get install libgstrtspserver-1.0-dev gstreamer1.0-rtsp

# 视频推流 bbb_sunflower_1080p_30fps_normal

gst-launch-1.0 filesrc location=test_media_files/bbb_sunflower_1080p_30fps_normal.flv ! \
    flvdemux name=demux \
    demux.video ! queue ! h264parse ! mux. \
    demux.audio ! queue ! aacparse ! mux. \
    flvmux name=mux ! \
    rtmp2sink location="rtmp://localhost/live/test"



# 视频推流 bbb_sunflower_1080p_30fps_normal

gst-launch-1.0 filesrc location=test_media_files/bbb_sunflower_1080p_30fps_normal.flv ! \
    flvdemux name=demux \
    demux.video ! queue ! h264parse ! sink. \
    demux.audio ! queue ! aacparse ! sink. \
    rtspclientsink location=rtsp://localhost:8554/live/test name=sink protocols=tcp



# 视频推流 pattern=ball

gst-launch-1.0 -v \
    videotestsrc is-live=true pattern=ball ! \
    videoconvert ! \
    x264enc tune=zerolatency bitrate=1000 ! \
    video/x-h264,profile=constrained-baseline ! h264parse ! mux. \
    rtspclientsink name=mux location="rtsp://localhost:8554/live/test" protocols=tcp


# 视频拉流播放
GST_DEBUG=3 gst-launch-1.0 \
  rtspsrc location=rtsp://127.0.0.1:8554/live/test protocols=tcp ! \
  rtph264depay ! \
  h264parse ! \
  decodebin ! \
  autovideosink

  #aac
  GST_DEBUG=3 gst-launch-1.0 -e \
  rtspsrc location=rtsp://127.0.0.1:8554/live/test protocols=tcp name=src \
  src. ! queue ! rtph264depay ! h264parse ! decodebin ! autovideosink \
  src. ! queue ! rtpmp4adepay ! aacparse ! decodebin ! autoaudiosink

GST_DEBUG=3 gst-launch-1.0 -e \
  rtspsrc location=rtsp://127.0.0.1:8554/live/test protocols=tcp name=src \
  src. ! queue ! rtph265depay ! h265parse ! decodebin ! autovideosink \
  src. ! queue ! rtpmp4adepay ! aacparse ! decodebin ! autoaudiosink

  gst-launch-1.0 playbin3 uri=rtsp://127.0.0.1:8554/live/test

  #laliu bofang
  GST_DEBUG=3 gst-launch-1.0 -v \
  rtmpsrc location="rtmp://127.0.0.1:1935/live/test live=1" ! \
  flvdemux name=demux \
  demux.video ! queue ! h264parse ! avdec_h264 ! autovideosink \
  demux.audio ! queue ! aacparse ! avdec_aac ! autoaudiosink


  GST_DEBUG=3 gst-launch-1.0 -v \
  rtmpsrc location="rtmp://127.0.0.1:1935/live/test live=1" ! \
  flvdemux name=demux \
  demux.video ! queue ! h264parse ! fakesink \
  demux.audio ! queue ! aacparse ! fakesink


