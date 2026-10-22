import os,sys,ctypes,json,datetime
source=os.path.abspath(sys.argv[1])
home=os.path.expanduser('~')
trash=os.path.join(home,'.Trash' if sys.platform=='darwin' else '.local/share/Trash/files')
if source in ('/',home) or source.startswith(trash+os.sep) or trash.startswith(source+os.sep):
 raise ValueError('Cannot trash this root, home, or trash location')
os.makedirs(trash,mode=0o700,exist_ok=True)
lib=ctypes.CDLL(None,use_errno=True)
def rename_exclusive(a,b):
 if sys.platform=='darwin': result=lib.renamex_np(os.fsencode(a),os.fsencode(b),4)
 else:
  fn=getattr(lib,'renameat2',None)
  if fn is None: raise RuntimeError('Safe no-replace rename is unavailable on this remote host')
  result=fn(-100,os.fsencode(a),-100,os.fsencode(b),1)
 if result: raise OSError(ctypes.get_errno(),os.strerror(ctypes.get_errno()))
name=os.path.basename(source)
for index in range(10000):
 target=os.path.join(trash,name if index==0 else name+'.'+str(index))
 if os.path.lexists(target): continue
 info=None
 if sys.platform!='darwin':
  import urllib.parse
  directory=os.path.join(os.path.dirname(trash),'info')
  os.makedirs(directory,mode=0o700,exist_ok=True)
  info=os.path.join(directory,os.path.basename(target)+'.trashinfo')
  try:
   with open(info,'x') as f:f.write('[Trash Info]\nPath='+urllib.parse.quote(source)+'\nDeletionDate='+datetime.datetime.now().strftime('%Y-%m-%dT%H:%M:%S')+'\n')
  except FileExistsError:continue
 try:rename_exclusive(source,target)
 except BaseException:
  if info:os.unlink(info)
  raise
 print(json.dumps(dict(trashed_to=target)))
 break
else:raise RuntimeError('Could not choose a unique trash path')
