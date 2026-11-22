
#include <stdio.h>
#include <string.h>
#include <stdint.h>
static int hex(const char*s,unsigned char*o,int n){for(int i=0;i<n;i++){unsigned v;if(sscanf(s+2*i,"%2x",&v)!=1)return -1;o[i]=v;}return 0;}
static void ph(const unsigned char*b,int n){for(int i=0;i<n;i++)printf("%02x",b[i]);}
int main(){
  char op[32],xs[64];unsigned fcw,sw0;
  while(scanf("%31s %x %x %63s",op,&fcw,&sw0,xs)==4){
    unsigned char x[16]={0},r[16]={0},env[32]={0},env0[32]={0};
    hex(xs,x,10);
    env0[0]=fcw&0xff;env0[1]=fcw>>8;env0[4]=sw0&0xff;env0[5]=sw0>>8;env0[8]=0xff;env0[9]=0xff;
    if(!strcmp(op,"fbld")){
      __asm__ volatile("fninit\n fldenv %[env0]\n fbld %[x]\n fnstenv %[env]\n fstpt %[r]\n fwait\n":[env]"=m"(env),[r]"=m"(r):[env0]"m"(env0),[x]"m"(x):"memory","st","st(1)");
    } else if(!strcmp(op,"fbstp")){
      __asm__ volatile("fninit\n fldenv %[env0]\n fldt %[x]\n fbstp %[r]\n fnstenv %[env]\n fwait\n":[env]"=m"(env),[r]"=m"(r):[env0]"m"(env0),[x]"m"(x):"memory","st","st(1)");
    } else continue;
    uint16_t sw=env[4]|(env[5]<<8), tw=env[8]|(env[9]<<8);
    printf("%s %04x %04x %s ",op,fcw,sw0,xs);ph(r,10);printf(" %04x %04x\n",sw,tw);
  }
  return 0;}
