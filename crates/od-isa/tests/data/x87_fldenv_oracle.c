#include <stdio.h>
#include <string.h>
#include <stdint.h>
static void ph(const unsigned char*b,int n){for(int i=0;i<n;i++)printf("%02x",b[i]);}
int main(){
  char xa[300],xb[100];
  while(scanf("%299s %99s",xa,xb)==2){
    unsigned char a[108],b[28],env[28],save[108],env2[28];
    for(int i=0;i<108;i++){unsigned v;sscanf(xa+2*i,"%2x",&v);a[i]=v;}
    for(int i=0;i<28;i++){unsigned v;sscanf(xb+2*i,"%2x",&v);b[i]=v;}
    __asm__ volatile("fninit\n frstor %[a]\n fldenv %[b]\n fnstenv %[env]\n fnsave %[save]\n fnstenv %[env2]\n fwait\n"
      :[env]"=m"(env),[save]"=m"(save),[env2]"=m"(env2):[a]"m"(a),[b]"m"(b):"memory");
    ph(a,108);printf(" ");ph(b,28);printf(" ");ph(env,28);printf(" ");ph(save,108);printf(" ");ph(env2,28);printf("\n");
  }
  return 0;}
