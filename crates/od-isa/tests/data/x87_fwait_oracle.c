#include <stdio.h>
#include <string.h>
#include <stdint.h>
/* S2-32f: the FWAIT spellings. GNU as emits 9B D9 30 for fstenv and 9B DD 30 for fsave. Same images as
   x87_env_oracle.c (one 108-byte FRSTOR image per input line); the answers are in
   x87_fwait_oracle_i5_3210m.txt as "in env save env2" in hex. */
static void ph(const unsigned char*b,int n){for(int i=0;i<n;i++)printf("%02x",b[i]);}
int main(){
  char xs[300];
  while(scanf("%299s",xs)==1){
    unsigned char in[108],env[28],save[108],env2[28];
    for(int i=0;i<108;i++){unsigned v;sscanf(xs+2*i,"%2x",&v);in[i]=v;}
    __asm__ volatile("fninit\n frstor %[in]\n fstenv %[env]\n fsave %[save]\n fstenv %[env2]\n fwait\n"
      :[env]"=m"(env),[save]"=m"(save),[env2]"=m"(env2):[in]"m"(in):"memory");
    ph(in,108);printf(" ");ph(env,28);printf(" ");ph(save,108);printf(" ");ph(env2,28);printf("\n");
  }
  return 0;}
