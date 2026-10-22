import os,sys,json,subprocess,shutil,tempfile,threading
root=sys.argv[1]
q=json.loads(sys.argv[2])
engine=q['engine']
if engine in ('auto','builtin'):
 engine='fd' if shutil.which('fd') else 'rg_files'
if engine=='fff':raise RuntimeError('Use an installed search plugin for remote FFF search')
if engine=='fd': args=['fd','--print0','--color','never','--fixed-strings','--no-require-git']+(['--type','f'] if q.get('files_only') else [])
elif engine in ('rg','rg_files'):args=['rg','--no-require-git','--glob','!.git/**']+(['--json','--fixed-strings','--max-columns','2000'] if engine=='rg' else ['--files','--null'])
else:raise ValueError('Unsupported remote search engine')
if q.get('hidden'):args+=['--hidden']
if not q.get('respect_gitignore',True):args+=['--no-ignore']
if engine in ('fd','rg'):args+=['--',q['text'],'.']
else:args+=['--','.']
limit=min(int(q['max_results']),10000)
results=[]
timed_out=threading.Event()
with tempfile.TemporaryFile() as errors:
 proc=subprocess.Popen(args,cwd=root,stdout=subprocess.PIPE,stderr=errors)
 def stop():timed_out.set();proc.kill()
 timer=threading.Timer(55,stop);timer.start()
 try:
  if engine=='rg':
   for line in proc.stdout:
    event=json.loads(line)
    if event.get('type')!='match':continue
    data=event['data'];path=data['path'].get('text')
    if path is None:continue
    absolute=os.path.normpath(os.path.join(root,path))
    if q.get('files_only') and (not os.path.isfile(absolute) or os.path.islink(absolute)):continue
    results.append(dict(path=absolute,name=os.path.basename(absolute),line=data.get('line_number'),snippet=data['lines'].get('text','').rstrip()))
    if len(results)>=limit:break
  else:
   pending=b''
   while len(results)<limit:
    chunk=os.read(proc.stdout.fileno(),8192)
    if not chunk:break
    pending+=chunk
    parts=pending.split(b'\0');pending=parts.pop()
    for part in parts:
     path=os.fsdecode(part)
     if engine=='rg_files' and q['text'].casefold() not in path.casefold():continue
     absolute=os.path.normpath(os.path.join(root,path))
     if q.get('files_only') and (not os.path.isfile(absolute) or os.path.islink(absolute)):continue
     results.append(dict(path=absolute,name=os.path.basename(absolute),line=None,snippet=path))
     if len(results)>=limit:break
 finally:
  timer.cancel()
  if proc.poll() is None:proc.kill()
  status=proc.wait();proc.stdout.close()
 if timed_out.is_set():raise TimeoutError('Remote search exceeded 55 seconds')
 if status not in (0,1) and len(results)<limit:
  errors.seek(0);raise RuntimeError(errors.read(16384).decode('utf8','replace'))
print(json.dumps(results))
