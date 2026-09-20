"""Real binary/worker integration checks against a controlled local HTTP server."""
import argparse, hashlib, http.server, json, os, pathlib, re, socketserver, subprocess, threading, time, tempfile, shutil

class Fixture(http.server.BaseHTTPRequestHandler):
    payload = bytes(range(256)) * 8192
    ranges = []
    gates = {}
    def log_message(self, *_): pass
    def do_HEAD(self):
        self.send_response(200); self.send_header('Content-Length', str(len(self.payload))); self.send_header('ETag', '"fixture-v1"'); self.end_headers()
    def do_GET(self):
        if self.path.startswith('/redirect'):
            self.send_response(302); self.send_header('Location', '/file.bin'); self.end_headers(); return
        if self.path.startswith('/missing'):
            self.send_error(404); return
        start = 0
        range_header = self.headers.get('Range')
        if range_header and not self.path.startswith('/no-range'):
            start = int(range_header.split('=')[1].split('-')[0]); self.ranges.append(start)
            self.send_response(206); self.send_header('Content-Range', f'bytes {start}-{len(self.payload)-1}/{len(self.payload)}')
        else: self.send_response(200)
        self.send_header('ETag', '"fixture-v1"')
        if not self.path.startswith('/unknown'): self.send_header('Content-Length', str(len(self.payload)-start))
        self.end_headers()
        try:
            for i in range(start, len(self.payload), 16384):
                self.wfile.write(self.payload[i:i+16384]); self.wfile.flush()
                if i == start and self.path in self.gates:
                    self.gates[self.path].wait(60)
                if 'slow' in self.path or 'no-range' in self.path: time.sleep(.025)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError): pass

def main():
    p=argparse.ArgumentParser();p.add_argument('--binary',required=True);p.add_argument('--artifacts',required=True);args=p.parse_args()
    binary=str(pathlib.Path(args.binary).resolve());root=pathlib.Path(args.artifacts).resolve();root.mkdir(parents=True,exist_ok=True)
    run_dir=pathlib.Path(tempfile.mkdtemp(prefix='run-',dir=root));profile=run_dir/'profile';output=run_dir/'Downloads with spaces — test';output.mkdir()
    report=[]
    def cli(*argv,check=True,timeout=40):
        result=subprocess.run([binary,'--data-dir',str(profile),'--json',*map(str,argv)],capture_output=True,text=True,encoding='utf-8',timeout=timeout)
        records=[json.loads(line) for line in result.stdout.splitlines() if line.strip()]
        if check and result.returncode: raise AssertionError((argv,result.returncode,records,result.stderr))
        return records,result
    def data(*argv): return cli(*argv)[0][-1]['data']
    def jobs(): return data('queue','list')
    def wait_job(job_id,status='completed',timeout=20):
        end=time.monotonic()+timeout
        while time.monotonic()<end:
            job=next(j for j in jobs() if j['id']==job_id)
            if job['status']==status: return job
            if job['status']=='failed' and status!='failed': raise AssertionError(job)
            time.sleep(.12)
        raise AssertionError(('Timed out',status,job))
    def submit(url,*more):
        route = url.split(base, 1)[-1]
        if 'slow' in route or 'no-range' in route:
            Fixture.gates[route] = threading.Event()
        return data('download',url,'--direct','--output',output,'--detach',*more)[0]
    class Server(socketserver.ThreadingMixIn,http.server.HTTPServer): daemon_threads=True
    server=Server(('127.0.0.1',0),Fixture);threading.Thread(target=server.serve_forever,daemon=True).start();base=f'http://127.0.0.1:{server.server_port}'
    try:
        dependencies=data('doctor');assert any(d['name']=='yt-dlp' for d in dependencies);report.append('JSON doctor and worker auto-start')
        job_id=submit(base+'/file.bin');job=wait_job(job_id);saved=pathlib.Path(job['files'][0]);assert hashlib.sha256(saved.read_bytes()).digest()==hashlib.sha256(Fixture.payload).digest();report.append('Direct HTTP output and SHA256 match')
        duplicate=wait_job(submit(base+'/file.bin'));assert duplicate['files'][0]!=str(saved);assert saved.exists();report.append('Duplicate destination preserves completed file')
        for route in ['/redirect','/unknown.bin']:
            j=wait_job(submit(base+route));assert pathlib.Path(j['files'][0]).read_bytes()==Fixture.payload
        report.append('Redirects and unknown content length')
        slow=submit(base+'/slow.bin');wait_job(slow,'active');time.sleep(.35);data('queue','pause',slow);wait_job(slow,'paused');Fixture.gates['/slow.bin'].set();time.sleep(.3);data('queue','resume',slow);j=wait_job(slow);assert pathlib.Path(j['files'][0]).read_bytes()==Fixture.payload;assert any(n>0 for n in Fixture.ranges);report.append('Pause/resume uses HTTP Range and preserves bytes')
        no_range=submit(base+'/no-range.bin');wait_job(no_range,'active');time.sleep(.3);data('queue','pause',no_range);Fixture.gates['/no-range.bin'].set();time.sleep(.3);data('queue','resume',no_range);j=wait_job(no_range,'failed');assert not j['resume_supported'];bad,proc=cli('queue','resume',no_range,check=False);assert proc.returncode!=0;data('queue','resume',no_range,'--allow-restart');j=wait_job(no_range);assert any(pathlib.Path(j['destination']).glob('*.preserved-*'));report.append('Unsupported resume requires confirmation and preserves partial')
        missing=submit(base+'/missing.bin');j=wait_job(missing,'failed');assert j['error'];report.append('HTTP errors become persisted failed jobs')
        slow=submit(base+'/slow-restart.bin');wait_job(slow,'active');time.sleep(.3);pid=data('worker','status')['pid'];
        if os.name=='nt':subprocess.run(['taskkill','/PID',str(pid),'/F'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True)
        else:os.kill(pid,9)
        Fixture.gates['/slow-restart.bin'].set();time.sleep(.4);j=next(j for j in jobs() if j['id']==slow);assert j['status']=='paused';data('queue','resume',slow);wait_job(slow);report.append('Worker crash recovery and reconnect')
        cookie=run_dir/'cookies.txt';cookie.write_text('# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t0\tsession\tsecret-value\n',encoding='utf-8');account=data('auth','import-cookies',cookie,'--name','fixture');assert account['cookies']==1;assert 'secret-value' not in json.dumps(data('auth','list'));report.append('Cookie import without secret disclosure')
        metadata=data('info',base+'/file.bin','--direct');assert metadata['size']==len(Fixture.payload);report.append('Metadata without download')
        batch=run_dir/'links.txt';batch.write_text(f'# fixture\n{base}/file.bin\n{base}/unknown.bin\n',encoding='utf-8');ids=data('batch',batch,'--direct','--output',output,'--detach');assert len(ids)==2
        for i in ids:wait_job(i)
        report.append('Batch downloads and concurrent queue')
        ffmpeg=next((d['path'] for d in dependencies if d['name']=='ffmpeg'),None)
        if ffmpeg:
            tone=run_dir/'tone.wav';subprocess.run([ffmpeg,'-hide_banner','-loglevel','error','-f','lavfi','-i','sine=frequency=440:duration=1','-y',str(tone)],check=True)
            ids=data('convert',tone,'--format','flac','--output',output,'--detach');j=wait_job(ids[0]);assert pathlib.Path(j['files'][0]).stat().st_size>0;report.append('Real FFmpeg conversion')
        library=data('library');assert library;export=run_dir/'library.json';data('library','--export',export);assert json.loads(export.read_text(encoding='utf-8'));report.append('Library listing and export')
        render=run_dir/'preview.ansi';subprocess.run([binary,'--data-dir',str(profile),'render',str(render)],check=True);assert 'Paste a URL' in re.sub(r'\x1b\[[0-9;]*m','',render.read_text(encoding='utf-8'));report.append('Dashboard rendering')
        data('worker','stop');time.sleep(.3);assert not data('worker','status')['running'];report.append('Graceful worker shutdown')
        result={'passed':len(report),'checks':report,'run_dir':str(run_dir),'platform':os.name};(root/'verification.json').write_text(json.dumps(result,indent=2),encoding='utf-8');print(json.dumps(result,indent=2))
    finally:
        cli('worker','stop',check=False);server.shutdown()
if __name__=='__main__':main()
