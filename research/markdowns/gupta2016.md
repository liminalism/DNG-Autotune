<!-- Converted from gupta2016.pdf — 6 pages -->

## Page 1

### Optik
j ou rn a l h o m e p a g e : w w w . e l s e v i e r . d e / i j l e o
### Minimum mean brightness error contrast enhancement of color
### images using adaptive gamma correction with color preserving
### framework
Bhupendra Gupta∗, Mayank Tiwari
Indian Institute of Information Technology, Design & Manufacturing Jabalpur, 482005 MP, India
| a r t i c l e | i n f o | a b s t r a c t |
|---|---|---|
| Article history: |  | In this paper, we propose an efficient method to enhance contrast of digital images. Image |
| Received 25 December 2014 |  | enhancement is a pre-processing step that improves efficiency of other image dealing applications |
| Accepted 11 October 2015 |  | as computer vision and pattern recognition. The proposed method enhances contrast and brightness |
a r t i c l e i n f o a b s t r a c t
Article history:
of   given   image   using   the   gamma   correction   and   weighted   probability   distribution   of   luminance
Experiments results   show   that   the   proposed   method   is   able   to   enhance   contrast   of   all   type   of   color
©   2015   Elsevier   GmbH.   All   rights
without much affecting its visual and color information.
mayanktiwariggits@gmail.com (M. Tiwari). An extension of BBHE has been proposed by Chen and URL: http://https://www.bhupendragupta.com (B. Gupta). in [11], known as ‘minimum mean brightness error bi-histogram
http://dx.doi.org/10.1016/j.ijleo.2015.10.068 0030-4026/© 2015 Elsevier GmbH. All rights reserved.

---

## Page 2

rth recursion the number of sub-histogram is 2r, where number of
recursion depends on choice of user. Also authors proved math-ematically that as r increases, the mean brightness of processed imageapproachestowardsthemeanbrightnessofinputimage. Sim et al. in [13], improved DSIHE into ‘recursive sub-image histogram equalization’ based contrast enhancement (RSIHE), by introduc-ing recursive segmentation in the similar manner as Chen and Ramil proposed in [12], this method is similar to RMSHE but it usesmedianvaluesinsteadofmeanvaluestodividehistograminto sub-histograms. Recently, S.C. Huang et al. [16], propose ‘efficient contrast enhancement using adaptive gamma correction with weighting distribution’ (AGCWD). This method is an automatic transforma-tion technique that improves the brightness of dimmed images via the gamma correction and probability distribution of lumi-nance pixels; also this method produces enhanced images of comparable or higher quality than those produced by other methods [16]. The organization of this work is as follows. After introduc-ing about research background in Section 1, Section 2 describes the AGCWD method. Section 3 explains the proposed method in details. Forevaluationofproposedmethodexperimentalresultsare shown in Section 4. Finally Section 5 summarizes and concludes the proposed work.
2. The AGCWD method
As mentioned earlier in [16], S.C. Huang et al. proposes ‘effi-cientcontrastenhancementusingadaptivegammacorrectionwith weighting distribution’, (AGCWD). The AGCWD method can pro-gressively increase the low intensity and avoid the significant decrement of the high intensity. The AGCWD method applies the weighting distribution (WD) function to slightly modify the statis-tical histogram and lessen the generation of adverse effects. The WD function is formulated as:
### (
### )˛
pdf (l) = pdf pdf(l) − pdfmin , w max pdfmax − pdfmin (1)
![Figure 2](gupta2016_p2_figure2.jpg)
where ˛ is the adjusted parameter, pdf max is the maximum pdf of the statistical histogram, and pdf min is the minimum pdf. Based on Eq. (1) the gamma parameter is given as:
 = 1 − cdf(l), (2)
where cdf is resultant cumulative distribution function calculated afternormalizingpdfw. Theadaptivegammacorrection(AGC)used by S.C. Huang et al. in [16] is given as:
### (
### )
### (
###  )1−cdf(l)
l l T(l) = (L − 1) = (L − 1) , (3)
$$ L − $$
1 L − 1
![Figure 1](gupta2016_p2_figure1.jpg)

![Figure 3](gupta2016_p2_figure3.jpg)
where (L−1) is the maximum intensity value of the image. It is clear that slight increase in cdf(l) will cause drastic change in intensity value l of the processed image.
TheresultantcdfcurveofAGCWD[16]methodisshownin It is clear from Fig. 1 that for ˛=1.2 and ˛=0.8 the resultant transformation curve is not increasing smoothly. To prove our claim (in image1 for transformation curve ˛=1.2) we have marked two intervals on x-axis (x1, x2); (x3, x4) and their corresponding intervals on y-axis (y1, y2); (y3, y4). The small interval (x1, transformed to sufficiently large interval (y1, y2) and in this sufficient weights will be assigned to Eq. (3). However on the hand due to non-smooth nature of the curve the large interval x4) is transformed to a small interval (y3, y4) and hence weights a large interval will be not sufficiently large. This may also lead loss of information in processed image. To solve this problem and to make the resultant transformation curve to be smoothly increasing with sufficient color preservation; weproposedanewmethodcalled‘adaptivegammacorrection color preserving framework’ (AGCCPF). The proposed method two-steps processing; initially it enhances contrast and brightness ofgivenimageusingmodifiedprobabilitydistributionofluminance pixels followed by the gamma correction. In second step it uses color preserving framework for color restoration. The objective of all histogram based contrast enhancement methods is to transform the histogram of an input image
Fig. 1. ResultanttransformationcurveofAGCWDmethod, afterapplyingweighting distribution function on image1, image2 and image3. Here x-axis shows gray imageandy-axisshowscorrespondingcumulativedistributionfunctionateach level.

---

## Page 3

image to a gray level distribution which is closer to uniform gray level distribution. Here, we use a histogram modification function for contrast enhancement. This function has the ability to control the level of contrast enhancement in the processed image. In histogram equalization we consider an image as a 2 dimen-sional array of gray levels. Let the (i, j) element of this array is X(i;j) be the intensity of (i, j) pixel of the image, where X(i;j) is from the L discrete gray levels denoted by {X0, X1, . ., XL−1}. Let H be the histogram of the input image and let Hu be the uniform histogram. The main idea is to obtain a modified histogram ´H from the input histogram H such that the difference between H´ and Hu is suffi-ciently small, keeping the ´H closer to H. Now it is an optimization problem and can be formalized as:
H´ = H + (1 − )H ,
u (4)
$$ where 0≤≤1. $$
InFig. 2, weareshowingmappingfunctionfordifferentvaluesof . Thismappingfunctionisnothingbutthecumulativedistribution function of resultant modified histogram. For =0.5 the mapping curve increases smoothly to the maximum value and this causes no over enhancement in the processed image. Now based on Eq. (4) the corresponding normalized probability mass function (PMF) can be given as:
H´ (l) pmf(l) =
, (5)
 H    ´
   H   ´   =   ∑ L − 1 H ´ ( k ).   Now corresponding
k = 0  
given   as:
∑ l
=
pmf   ( k ) .  
(6)
k = 0
![Figure 1](gupta2016_p3_figure1.jpg)
correction using Eqs. (2) and (3). By the histogram modification function we can adjust the cdf of input image so that the resultant cdf is smoothly increasing and the color information of input image and applying it in processed hence no over enhancement will occur. However when we deal image. Now we can restore color information in processed withcolorimagesthenouraimisnotonlytoenhancetheircontrast by the following: but also to preserve their color information up to some extent. For PRC = ı(INT ) + (1 − ı)(INP preservation of color information in processed image, we are using IMG IMG IMG),
Fig. 3. Set of images used in the proposed work, images are given names as image1, image2, up to image12.

---

## Page 4

eter0≤ı≤1. Itisclearthatforı=0PRC =INP IMG IMG andinthiscase there would be maximum color preservation without any contrast enhancement in the PRC . For ı=1 PRC =INT IMG IMG IMG and in this case there would be minimum color preservation with contrast enhancement. In the proposed work we are working with color images using theRGBcolormodel. TheRGBcolormodelisanadditivecolormodel in which red, green, and blue light are added together in various ways to reproduce a broad array of colors. The name of the model comes from the initials of the three additive primary colors, red, green, and blue. In implementation part of proposed algorithm, we are applying the algorithm on each color channel of given color image, after that we are combining all three channels to form the processed image.
4. Experimental results
In this section, we demonstrate performance of the proposed method in comparison with some existing HE based contrast enhancement methods, like HE, RMSHE, RSIHE, RSWHE and AGCWD. For RMSHE, RSIHE and RSWHE we are using recursion level r=1. As test images we are taking images having poor
Fig. 4. Enhancement results of various methods for image2. Here (a) input image, (b) result of HE, (c) result of RMSHE (r=1), (d) result of RSIHE (r=1), (e) result of CLAHE (AMBE=6.514), (f) result of AGCWD (˛=0.8), (g) proposed method (=0.4 and ı=0.4) (AMBE=0.011), (h) proposed method (=0.25 and ı=0.4).
cloud cover, intensity of sunshine, etc. Fig. 3 shows set of images, used by us in the proposed work. Fig. 4showsenhancementresultsofvariousmethodsforimage Theimage2 containsscenewithanobjectandclearskybackground. The HE method produces over enhancement and noise enhance-ment too; in the processed image (b). Results of other methods such as RMSHE and RSIHE are able to enhance the contrast image at the cost of natural appearance’s loss. The CLAHE method is producing better results but this method is not able to maintain the mean brightness in the processed image. The AGCWD method produces good results but in this case color-information of the in the background is washed out. On the other hand the AGCCPF method is able to enhance contrast of given image without affecting it color-information and natural appearance. Table1showsresultsofAbsoluteMeanBrightnessError(AMBE). AMBE is used to calculate difference in mean brightness between two images. Mathematical expression to calculate AMBE between two images is given as:
AMBE = |XM − YM|,
whereXM andYM aremeanbrightnessofinputandprocessed respectively. Based on results of Table 1, we observe that the proposed methodhasleastvaluesinall12imagesascomparetoothermeth-ods. Further if we look at last row of Table 1, which shows average
Fig. 5. Enhancement results of proposed method for image1, image12, image image9. Here first column shows the input images second column shows sponding processed images. For these 4 images we have considered =0.4 ı=0.4.

---

## Page 5

Image name HE RMSHE RSIHE CLAHE AGCWD
56.834 8.750 4.380 17.055
image1 33.376
image 2   29.170  
3   22.077  
4   32.549  
image 5   40.720  
6   33.263  
image 7   36.037  
image 8   48.232  
image 9   27.806  
10   34.718  
image 11   31.385  
image 12   30.044  
43.608 8.190 11.062 12.858
Average 33.281
# results
# of AMBE then we find that the proposed method has least where MSE is Mean Square Error and it is defined as:
# average AMBE values among other methods. This shows that our
method is able to maintain mean brightness of input image in the 1 ∑M∑N 2
# MSE =
# |X(i, j) − Y(i, j)| ,
# processed image.
# MN
Table 2 shows results of Peak Signal to Noise Ratio matrix. Peak i=1 j=1
# Signal to Noise Ratio (PSNR) is widely used matrix which is used to
# whereX(i, j)andY(i, j)denotesintensityatlocation(i, j)inthe
# measurecontrastenhancement[13,14,17,18]. GreaterPSNRvalues
# and processed image respectively.
# indicate better the image quality [14]. To calculate PSNR between
# Table 2 proves that proposed method enhances more
# twoimages(eachimageishavingLdiscretegraylevelsintherange
# than HE, RMSHE, RSIHE, CLAHE and AGCWD (as higher PSNR
# {X0, X1, . ., XL−1}), mathematical expression is given as:
# indicates better contrast enhancement [13,14,17,18]). On
2
# (L − 1)
# PSNR = 10log
# 10 MSE , (9)
Fig. 6. Enhancement results of proposed method for image4, image6, image7 and Fig. 7. Enhancement results of proposed method for image2, image10, image image8. Here first column shows the input images second column shows corre- image3. Here first column shows the input images second column shows sponding processed images. For these 4 images we have considered =0.4 and sponding processed images. For these 4 images we have considered ı=0.4. ı=0.4.

---

## Page 6

| Image name | HE | RMSHE | RSIHE | CLAHE | AGCWD | AGCCPF |
|---|---|---|---|---|---|---|
| image 1 | 11.43 | 21.25 | 25.25 | 19.46 | 14.92 | 23.65 |
| image 2 | 13.65 | 17.67 | 19.53 | 21.40 | 16.53 | 24.62 |
| 3 | 10.31 | 15.80 | 18.88 | 22.01 | 18.90 | 26.88 |
| 4 | 13.49 | 21.68 | 25.36 | 19.96 | 15.21 | 21.83 |
| image 5 | 9.98 | 23.19 | 17.66 | 18.69 | 13.18 | 23.18 |
| 6 | 23.04 | 21.68 | 22.63 | 17.47 | 16.29 | 23.42 |
| image 7 | 13.66 | 20.33 | 22.73 | 17.81 | 15.20 | 26.05 |
| image 8 | 5.78 | 13.95 | 9.90 | 18.53 | 11.45 | 31.29 |
| image 9 | 18.03 | 18.28 | 18.19 | 21.73 | 18.26 | 27.96 |
| 10 | 12.71 | 22.28 | 26.94 | 19.27 | 15.46 | 26.70 |
| image 11 | 13.94 | 20.31 | 23.16 | 18.17 | 15.78 | 22.80 |
| image 12 | 19.39 | 20.15 | 19.18 | 16.99 | 17.37 | 26.14 |
| Average | 13.79 | 19.71 | 20.79 | 19.29 | 15.71 | 25.38 |
11.43 21.25 25.25 19.46 14.92 image1 23.65 13.65 17.67 19.53 21.40 16.53 image2 24.62 10.31 15.80 18.88 22.01 18.90 3 26.88 13.49 21.68 25.36 19.96 15.21 4 21.83 9.98 23.19 17.66 18.69 13.18 image5 23.18 23.04 21.68 22.63 17.47 16.29 6 23.42 13.66 20.33 22.73 17.81 15.20 image7 26.05 5.78 13.95 9.90 18.53 11.45 image8 31.29 18.03 18.28 18.19 21.73 18.26 image9 27.96 12.71 22.28 26.94 19.27 15.46 10 26.70 13.94 20.31 23.16 18.17 15.78 image11 22.80 19.39 20.15 19.18 16.99 17.37 image12 26.14
13.79 19.71 20.79 19.29 15.71 Average 25.38
## of results of Tables 1 and 2 we can conclude that the AGCCPF is a
[6] A. Polesel, G. Ramponi, V. Mathews, Image enhancement via adaptive unsharp goodmethodforcontrastenhancementwithmaximumbrightness masking, IEEE Trans. Image Process. 9 (March (3)) (2000) 505–510. [7] Y.S. Chiu, F.C. Cheng, S.C. Huang, Efficientcontrastenhancementusingadaptive
## preservation and color preservation.
gamma correction and cumulative intensity distribution, in: Proc. IEEE
## In the following Figs. 5–7 we are showing results of proposed
Syst. Man Cybern., 2011, pp. 2946–2950. method on all 12 images. [8] K. Zuiderveld, Contrast limited adaptive histogram equalization, in: P.S. bert (Ed.), Graphics Gems IV, Academic Press, Cambridge, MA, 1994, 474–485 (chapter VIII).
## 5. Conclusion
[9] Y. Kim, Contrastenhancementusingbrightnesspreservingbi-histogram ization, IEEE Trans. Consum. Electron. 43 (February (1)) (1997) 1–8. [10] Y. Wan, Q. Chen, B. Zhang, Image enhancement based on equal area dualistic
## The AGCCPF method provides optimum contrast enhancement
sub-image histogram equalization method, IEEE Trans. Consum. Electron.
## while preserving the brightness of given color image and hence
(February (1)) (1999) 68–75. this method is suitable for all types of color images. Experimental [11] S. Chen, A.R. Ramli, Minimum mean brightness error bi-histogram equaliza-tion in contrast enhancement, IEEE Trans. Consum. Electron. 49 (4)
## results show that AMBE of the proposed method is less in compar-
1310–1319.
## ison of other methods. Also PSNR of the proposed method is better
[12] S. Chen, R. Ramli, Contrast enhancement using recursive mean-separate fromHE, RMSHE, RSIHE, CLAHEandAGCWD. Onthebasisofanaly- togram equalization for scalable brightness preservation, IEEE Trans. Consum. sis of these two metrics, it is clear that AGCCPF preserves the input Electron. 49 (4) (2003) 1301–1309. [13] K.S. Sim, C.P. Tso, Y. Tan, Recursive sub-image histogram equalization
## image brightness more accurately and gives processed image with
applied to gray-scale images, Pattern Recognit. Lett. 28 (July (10)) good contrast enhancement. 1209–1221. [14] M. Kim, M.G. Chung, Recursively separated and weighted histogram ization for brightness preservation and contrast enhancement, IEEE
## References
Consum. Electron. 54 (3) (2008 Aug) 1389–1397. [15] T. Celik, T. Tjahjadi, Automatic image equalization and contrast enhancement [1] T. Arici, S. Dikbas, Y. Altunbasak, A histogram modification framework and its using Gaussian mixture modeling, IEEE Trans. Image Process. 21 (January application for image contrast enhancement, IEEE Trans. Image Process. 18 (2012) 145–156. (September (9)) (2009) 1921–1935. [16] S.C. Huang, F.C. Cheng, Y.S. Chiu, Efficientcontrastenhancementusingadaptive [2] A. Beghdadi, A.L. Negrate, Contrast enhancement technique based on local gamma correction with weighting distribution, IEEE Trans. Image Process. detection of edges, Comput. Vis. Graph. Image Process. 46 (May (2)) (1989) (March (3)) (2013) 1032–1041. 162–174. [17] S. Mohan, M. Ravishankar, Modifiedcontrastlimitedadaptivehistogram [3] H.D. Cheng, H.J. Xu, A novel fuzzy logic approach to contrast enhancement, ization based on local contrast enhancement for mammogram images, Pattern Recognit. 33 (May (5)) (2000) 809–819. Mobile Communication and Power Engineering Communications in Computer [4] J. Tang, X. Liu, Q. Sun, A direct image contrast enhancement algorithm in the and Information Science, vol. 296, 2013, pp. 397–403. wavelet domain for screening mammograms, IEEE J. Sel. Top. Signal Process. 3 [18] S. Mohan, M. Ravishankar, Optimized histogram based contrast (February (1)) (2009) 74–80. enhancement for mammogram images, short paper, ACEEE Int. J. Inf. Technol. [5] R. Sherrier, G. Johnson, Regionallyadaptivehistogramequalizationofthechest, 3 (March (1)) (2013). IEEE Trans. Med. Imaging 6 (January (1)) (1987) 1–7.

### Extracted Citations (JSON)
```json
[
  {
    "id": "15",
    "text": "T. Celik, T. Tjahjadi, Automatic image equalization and contrast enhancement"
  },
  {
    "id": "1",
    "text": "T. Arici, S. Dikbas, Y. Altunbasak, A histogram modification framework and its using Gaussian mixture modeling, IEEE Trans. Image Process. 21 (January application for image contrast enhancement, IEEE Trans. Image Process. 18 (2012) 145–156. (September (9)) (2009) 1921–1935. [16] S.C. Huang, F.C. Cheng, Y.S. Chiu, Efficientcontrastenhancementusingadaptive"
  },
  {
    "id": "2",
    "text": "A. Beghdadi, A.L. Negrate, Contrast enhancement technique based on local gamma correction with weighting distribution, IEEE Trans. Image Process. detection of edges, Comput. Vis. Graph. Image Process. 46 (May (2)) (1989) (March (3)) (2013) 1032–1041. 162–174. [17] S. Mohan, M. Ravishankar, Modifiedcontrastlimitedadaptivehistogram"
  },
  {
    "id": "3",
    "text": "H.D. Cheng, H.J. Xu, A novel fuzzy logic approach to contrast enhancement, ization based on local contrast enhancement for mammogram images, Pattern Recognit. 33 (May (5)) (2000) 809–819. Mobile Communication and Power Engineering Communications in Computer"
  },
  {
    "id": "4",
    "text": "J. Tang, X. Liu, Q. Sun, A direct image contrast enhancement algorithm in the and Information Science, vol. 296, 2013, pp. 397–403. wavelet domain for screening mammograms, IEEE J. Sel. Top. Signal Process. 3 [18] S. Mohan, M. Ravishankar, Optimized histogram based contrast (February (1)) (2009) 74–80. enhancement for mammogram images, short paper, ACEEE Int. J. Inf. Technol."
  },
  {
    "id": "5",
    "text": "R. Sherrier, G. Johnson, Regionallyadaptivehistogramequalizationofthechest, 3 (March (1)) (2013). IEEE Trans. Med. Imaging 6 (January (1)) (1987) 1–7."
  }
]
```