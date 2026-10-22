"""Ephemeral CPU embeddings: process exit releases model memory after each operation."""
import json, sys
import torch
from transformers import AutoTokenizer, AutoModel
torch.set_num_threads(2)
texts=json.load(sys.stdin)
tokenizer=AutoTokenizer.from_pretrained(sys.argv[1],local_files_only=True)
model=AutoModel.from_pretrained(sys.argv[1],local_files_only=True).eval()
vectors=[]
with torch.inference_mode():
 for start in range(0,len(texts),8):
  batch=tokenizer(texts[start:start+8],padding=True,truncation=True,max_length=512,return_tensors='pt')
  output=model(**batch).last_hidden_state[:,0]
  vectors.extend(torch.nn.functional.normalize(output,p=2,dim=1).tolist())
print(json.dumps(vectors))
